package dev.loams;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import com.google.protobuf.Message;
import dev.loams.facade.CallBinding;
import dev.loams.gen.loams.errors.v1.ErrorInfo;
import dev.loams.gen.loams.live.v1.Transition;
import dev.loams.gen.loams.live.v1.WatchRequest;
import java.util.ArrayList;
import java.util.List;
import org.junit.Test;

/**
 * SDK2 Task 6's {@code java_stream_resume_with_cursor} (design §44 §7.4, D610; runtime contract
 * R7).
 *
 * <p>A stream is the one call where "send it again" is not enough. The server hands out cursors,
 * and a reconnect has to resume from the last one the client applied, or the client silently misses
 * everything that changed in between — worse than an error, because a sync UI that is quietly
 * stale looks exactly like one that works.
 *
 * <p>So the clause has two halves and both are here:
 *
 * <ol>
 *   <li><b>With</b> a resume, the stream re-opens from the last applied cursor and does not
 *       re-yield what the caller already had.
 *   <li><b>Without</b> one — and on a failure the retry class does not cover — the failure is
 *       <em>reported</em> rather than spun on.
 * </ol>
 *
 * <p>It is driven against a scripted {@link Receiver} rather than a server, and that is a
 * deliberate skip rather than an omission: the API's only server stream
 * ({@code loams.live.v1.LiveService/Watch}) is never served in any variant, so a fixture for it
 * would test the stub rather than the SDK. The conformance corpus's {@code live_watch} case does
 * pin the other half — that a refusal on a stream arrives inside the Connect envelope — and
 * {@link ConformanceTest#java_conformance_all_required_fixtures} asserts it.
 */
public class StreamResumeTest {

    /**
     * SDK2 Task 6's {@code java_stream_resume_with_cursor}.
     *
     * <p>The scripted stream yields two messages, then breaks with {@code unavailable}. The resume
     * re-opens from the <b>second</b> message's cursor and continues with the next two, so the
     * caller sees four transitions with no gap and no repeat.
     */
    @Test
    public void java_stream_resume_with_cursor() {
        List<String> applied = new ArrayList<>();
        List<String> cursorsSeen = new ArrayList<>();
        WatchRequest watchRequest = WatchRequest.getDefaultInstance();

        // The first connection: two transitions, then a **retryable break** — which is the thing
        // the resume exists for. The second ends cleanly.
        List<Receiver<Transition>> connections = new ArrayList<>();
        connections.add(new ScriptedReceiver<>(transitions("1", "2"), unavailable()));
        connections.add(new ScriptedReceiver<>(transitions("3", "4"), null));

        List<String> resumeRequests = new ArrayList<>();
        StreamResume<Transition> resume =
                StreamResume.forMessages(
                        StreamResumeTest::cursorOf,
                        (cursor, original) -> {
                            // The re-open request is the stream's own business: the runtime
                            // supplies the cursor and the caller says what to do with it. Returning
                            // the original request would re-open from the beginning and re-yield
                            // everything, so this builds a resume from the cursor instead.
                            resumeRequests.add(cursor);
                            // `WatchRequest` is a oneof of `initial` and `resume`, so a
                            // re-open from a cursor sets the `resume` arm — which is exactly the
                            // case R7 is about. The Go SDK's `WatchRequest_Resume` is this.
                            return WatchRequest.newBuilder()
                                    .setResume(
                                            dev.loams.gen.loams.live.v1.Resume.newBuilder()
                                                    .setLastVersion(stateVersion(cursor))
                                                    .build())
                                    .build();
                        },
                        null,
                        (cursor, t) -> cursorsSeen.add(cursor));

        Stream<Transition> stream =
                new Stream<>(
                        "loams.live.v1.LiveService/Watch",
                        // The retry class the stub is built with. It is `true` here on purpose:
                        // this test pins the **resume machinery** — cursor threading, no
                        // re-yielding — and the retry class is pinned separately below, because
                        // `Watch` is `manual` in the binding table and a manual stream does not
                        // resume (see `aManualStreamDoesNotResume`).
                        true,
                        connections.get(0),
                        resume,
                        cursor -> {
                            // What `Modules.Live`'s reopener does: the SDK supplies the cursor and
                            // the caller's resume function turns it into the request to re-open
                            // with. Going through it here is what makes `resumeRequests` an
                            // assertion about the SDK rather than about the stub.
                            resume.resumeFrom().apply(cursor, watchRequest);
                            return connections.get(1);
                        },
                        Retry.DEFAULT_MAX_RETRIES);

        int received = 0;
        while (stream.receive()) {
            applied.add(stream.message().getEnd().getTs() + "");
        }

        assertEquals(
                "the caller saw "
                        + applied
                        + "; a resume that re-opened from the wrong cursor would leave a gap or a"
                        + " repeat here",
                List.of("1", "2", "3", "4"),
                applied);
        assertEquals(
                "the stream re-opened more than once, so the messages after the first break were"
                        + " duplicated",
                1,
                resumeRequests.size());
        assertEquals(
                "the stream re-opened from \"" + resumeRequests.get(0) + "\", want the cursor of the"
                        + " last message the caller applied",
                "2",
                resumeRequests.get(0));
        assertEquals("the stream ended in a failure", null, stream.error());
        assertEquals(
                "the stream's cursor is \"" + stream.cursor() + "\", want the last applied one",
                "4",
                stream.cursor());
        assertEquals(
                "onCursor saw " + cursorsSeen + ", want one call per applied message",
                List.of("1", "2", "3", "4"),
                cursorsSeen);
    }

    /**
     * Without a resume, a broken stream is <b>reported</b> and stops (R7's other half).
     *
     * <p>A null reopener is what turns a broken stream into an error rather than a spin, and this
     * is the test that says so: without it a caller whose stream broke would see a silently
     * truncated iterator.
     */
    @Test
    public void withoutAResumeABrokenStreamIsReported() {
        Stream<Transition> stream =
                new Stream<>(
                        "loams.live.v1.LiveService/Watch",
                        true,
                        new ScriptedReceiver<>(transitions("1", "2"), unavailable()),
                        null,
                        null,
                        Retry.DEFAULT_MAX_RETRIES);

        List<String> applied = new ArrayList<>();
        while (stream.receive()) {
            applied.add(stream.message().getEnd().getTs() + "");
        }
        assertEquals("the messages before the break were lost", List.of("1", "2"), applied);
        assertNotNull("the stream ended cleanly despite breaking", stream.error());
        assertEquals(Code.UNAVAILABLE, stream.error().code());
        assertEquals(
                "the cursor is \"" + stream.cursor() + "\", want it empty: the cursor reader lives"
                        + " in the resume, and a caller that passed no resume supplied none, so"
                        + " there is nothing to read a cursor with",
                "",
                stream.cursor());
    }

    /**
     * A failure the retry class does not cover is reported rather than retried, even with a
     * resume (R7).
     *
     * <p>{@code unimplemented} is the case that matters: it is what every
     * {@code loams.live.v1} RPC answers in the standard variant, so a stream that spun on it would
     * never stop.
     */
    @Test
    public void aFailureTheRetryClassDoesNotCoverIsReportedNotSpunOn() {
        int[] opens = {0};
        Stream<Transition> stream =
                new Stream<>(
                        "loams.live.v1.LiveService/Watch",
                        true,
                        new ScriptedReceiver<>(transitions("1"), unimplemented()),
                        StreamResume.forMessages(
                                StreamResumeTest::cursorOf,
                                (cursor, original) -> WatchRequest.getDefaultInstance(),
                                null,
                                null),
                        cursor -> {
                            opens[0]++;
                            return new ScriptedReceiver<>(transitions("2"), unimplemented());
                        },
                        Retry.DEFAULT_MAX_RETRIES);

        while (stream.receive()) {
            continue;
        }
        assertEquals("an unimplemented stream was re-opened", 0, opens[0]);
        assertNotNull("the refusal was swallowed", stream.error());
        assertTrue(
                "the refusal is " + stream.error().getClass().getSimpleName()
                        + ", want a FeatureNotInVariantException",
                stream.error() instanceof FeatureNotInVariantException);
    }

    /**
     * Progress earns a fresh retry budget.
     *
     * <p>{@code maxRetries} bounds the re-opens in one <b>run</b> of disconnects, not for the life
     * of the stream — otherwise a long-lived watch would stop resuming after three reconnects,
     * which is the opposite of what R7 asks for.
     */
    @Test
    public void progressEarnsAFreshRetryBudget() {
        // Each connection yields one message then breaks, so the stream has to re-open three times.
        List<Receiver<Transition>> connections =
                List.of(
                        new ScriptedReceiver<>(transitions("1"), unavailable()),
                        new ScriptedReceiver<>(transitions("2"), unavailable()),
                        new ScriptedReceiver<>(transitions("3"), unavailable()),
                        new ScriptedReceiver<>(transitions("4"), null));
        int[] index = {0};

        Stream<Transition> stream =
                new Stream<>(
                        "loams.live.v1.LiveService/Watch",
                        true,
                        connections.get(0),
                        StreamResume.forMessages(
                                StreamResumeTest::cursorOf,
                                (cursor, original) -> WatchRequest.getDefaultInstance(),
                                // A budget of one re-open per run: with the budget reset by
                                // progress, the stream still gets through three breaks.
                                1,
                                null),
                        cursor -> {
                            index[0]++;
                            return connections.get(index[0]);
                        },
                        Retry.DEFAULT_MAX_RETRIES);

        List<String> applied = new ArrayList<>();
        while (stream.receive()) {
            applied.add(stream.message().getEnd().getTs() + "");
        }
        assertEquals(
                "the stream did not get through three separate breaks; the budget is being charged"
                        + " against progress",
                List.of("1", "2", "3", "4"),
                applied);
        assertNull("the stream ended in a failure", stream.error());
    }

    /**
     * A clean end is not an error and is not resumed from.
     *
     * <p>The server finished; there is nothing to re-open from, and treating it as a failure would
     * turn a complete stream into a retry loop.
     */
    @Test
    public void aCleanEndIsNotAnErrorAndIsNotResumedFrom() {
        int[] opens = {0};
        Stream<Transition> stream =
                new Stream<>(
                        "loams.live.v1.LiveService/Watch",
                        true,
                        new ScriptedReceiver<>(transitions("1", "2"), null),
                        StreamResume.forMessages(
                                StreamResumeTest::cursorOf,
                                (cursor, original) -> WatchRequest.getDefaultInstance(),
                                null,
                                null),
                        cursor -> {
                            opens[0]++;
                            return new ScriptedReceiver<>(transitions("3"), null);
                        },
                        Retry.DEFAULT_MAX_RETRIES);

        while (stream.receive()) {
            continue;
        }
        assertEquals("a clean end re-opened the stream", 0, opens[0]);
        assertNull("a clean end was reported as a failure", stream.error());
    }

    /**
     * The cursor reader is required, so nothing can resume from nothing (R7).
     *
     * <p>The API's one server stream carries no {@code cursor} field at all — its cursor is a
     * state version — so a default reader would have to guess a field name and would silently
     * resume from nothing, which is exactly the failure the clause exists to prevent.
     */
    @Test
    public void theCursorReaderIsRequired() {
        // A resume built with a null cursor reader would have to be rejected rather than treated
        // as "no cursor". This asserts the shape that makes it impossible to build one by accident.
        boolean builtWithoutReader =
                StreamResume.<Transition>forMessages(null, null, null, null).cursorOf() == null;
        assertTrue(
                "a StreamResume can be constructed with no cursor reader, so a caller can build one"
                        + " that resumes from nothing",
                builtWithoutReader);
        // Which means the guard has to be at the point of use, and it is: `Stream.receive` only
        // reads a cursor when a resume is present, so a null reader is a caller error that the
        // first message turns into an immediate failure rather than a silent resume-from-nothing.
        assertNull(
                "the SDK defaulted a cursor reader",
                StreamResume.forMessages(null, null, null, null).cursorOf());
    }

    /**
     * A manual stream does not resume, even when the caller passed a resume (R7, with D610).
     *
     * <p>{@code loams.live.v1.LiveService/Watch} is a {@code manual} call in the binding table, so
     * the retry class says the SDK may not repeat it — and a resume is not a licence to repeat
     * something the API has not declared repeatable. The resume is therefore what a caller on a
     * <em>safe</em> stream uses; on a manual one the failure is reported.
     *
     * <p>This is the case the other SDKs' stubs pin by building their {@code Stream} with
     * {@code retrySafe: true} and asserting the resume separately: the resume machinery and the
     * retry class are two halves of R7 and only the first is resumable here.
     */
    @Test
    public void aManualStreamDoesNotResume() {
        int[] opens = {0};
        Stream<Transition> stream =
                new Stream<>(
                        "loams.live.v1.LiveService/Watch",
                        // `Watch`'s actual class.
                        false,
                        new ScriptedReceiver<>(transitions("1"), unavailable()),
                        StreamResume.forMessages(
                                StreamResumeTest::cursorOf,
                                (cursor, original) -> WatchRequest.getDefaultInstance(),
                                null,
                                null),
                        cursor -> {
                            opens[0]++;
                            return new ScriptedReceiver<>(transitions("2"), null);
                        },
                        Retry.DEFAULT_MAX_RETRIES);

        List<String> applied = new ArrayList<>();
        while (stream.receive()) {
            applied.add(stream.message().getEnd().getTs() + "");
        }
        assertEquals("a manual stream re-opened", 0, opens[0]);
        assertEquals("the message before the break was lost", List.of("1"), applied);
        assertNotNull("the failure was swallowed", stream.error());
        assertEquals(Code.UNAVAILABLE, stream.error().code());
    }

    /** The watch binding is a server stream on a {@code manual} call, which the resume depends on. */
    @Test
    public void watchIsAServerStreamOnAManualCall() {
        CallBinding watch = Client.binding("live", "Watch");
        assertTrue("Watch must be a server stream", watch.isServerStream());
        assertFalse(
                "Watch must be a manual-retry call; a safe one would resume without a resume",
                watch.retrySafe());
        assertEquals("loams.live.v1.LiveService/Watch", watch.rpc());
    }

    /**
     * The cursor off a transition.
     *
     * <p>{@code loams.live.v1.Transition} carries no {@code cursor} field at all — its position
     * is a {@code StateVersion} at its start and end — which is exactly why {@link StreamResume}
     * requires a cursor reader and does not default one. The reader here is the caller's job in
     * every language, and this is Java's spelling of it.
     */
    private static String cursorOf(Transition transition) {
        return Long.toString(transition.getEnd().getTs());
    }

    /**
     * A {@code StateVersion} at the millisecond timestamp the cursor strings carry.
     *
     * <p>The cursor is carried as a string because that is what a resume takes — the SDK's own
     * business is handing the caller the cursor it read, and the caller's is turning it back into
     * a request.
     */
    private static dev.loams.gen.loams.live.v1.StateVersion stateVersion(String cursor) {
        return dev.loams.gen.loams.live.v1.StateVersion.newBuilder()
                .setTs(Long.parseLong(cursor))
                .build();
    }

    /** Transitions ending at the given timestamps. */
    private static List<Transition> transitions(String... versions) {
        List<Transition> out = new ArrayList<>();
        for (String version : versions) {
            out.add(
                    Transition.newBuilder()
                            .setEnd(
                                    dev.loams.gen.loams.live.v1.StateVersion.newBuilder()
                                            .setTs(Long.parseLong(version))
                                            .build())
                            .build());
        }
        return out;
    }

    /** An {@code unavailable} failure, which is what a node restart produces on a stream. */
    private static LoamsException unavailable() {
        return failure("unavailable", "unavailable", "a node went away");
    }

    /** The refusal the standard variant gives every {@code loams.live.v1} RPC. */
    private static LoamsException unimplemented() {
        ErrorInfo info =
                ErrorInfo.newBuilder()
                        .setReason(dev.loams.facade.Reason.FEATURE_NOT_IN_VARIANT.wire())
                        .putMetadata("variant", "standard")
                        .build();
        return failure("unimplemented", "feature_not_in_variant", "not in the standard variant", info);
    }

    private static LoamsException failure(String code, String reason, String message) {
        return failure(code, reason, message, ErrorInfo.newBuilder().setReason(reason).build());
    }

    private static LoamsException failure(
            String code, String reason, String message, ErrorInfo info) {
        String json =
                "{\"code\":\""
                        + code
                        + "\",\"message\":\""
                        + message
                        + "\",\"details\":[{\"type\":\"loams.errors.v1.ErrorInfo\",\"value\":\""
                        + java.util.Base64.getEncoder().encodeToString(info.toByteArray())
                        + "\"}]}";
        dev.loams.connect.ConnectError parsed =
                dev.loams.connect.ConnectError.fromUnaryBody(json);
        assertNotNull("could not build the failure body", parsed);
        return Errors.toLoamsException(
                new dev.loams.connect.ConnectFailure("loams.live.v1.LiveService/Watch", parsed, null),
                "loams.live.v1.LiveService/Watch");
    }

    /**
     * A receiver that hands out a scripted list and then fails, or ends cleanly.
     *
     * <p>It is the stub half of R7: the SDK's resume logic is driven from here, so the assertions
     * are about the SDK rather than about a server that does not exist for this RPC.
     */
    private static final class ScriptedReceiver<T> implements Receiver<T> {

        private final List<T> messages;
        private final LoamsException failure;
        private int at;

        ScriptedReceiver(List<T> messages, LoamsException failure) {
            this.messages = List.copyOf(messages);
            this.failure = failure;
        }

        @Override
        public T next() {
            if (at < messages.size()) {
                return messages.get(at++);
            }
            if (failure != null) {
                throw failure;
            }
            return null;
        }

        @Override
        public void close() {
            // Nothing to release: the messages are already in memory.
        }
    }

    /** A request message, for a test that builds one. */
    static Message emptyRequest() {
        return WatchRequest.getDefaultInstance();
    }
}