package dev.loams;

import dev.loams.internal.Json;
import java.io.IOException;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.function.Supplier;

/**
 * The RFC 8693 token exchange a person signed in through an OIDC provider needs (design §44
 * §7.4, D608).
 *
 * <p>The instance's {@code /oauth/token} protocol endpoint takes the identity token and answers
 * with a Loams access token, which is then cached until the server says it expired — the caching
 * and the refresh-once-and-retry behaviour are {@link RefreshingTokenSource}'s, which this owns.
 *
 * <p><b>Not exercised by the conformance suite.</b> The instance serves no OAuth endpoint yet (the
 * auth plan, MT, and API1 Task 7 build it), so this is written to the documented request and
 * response and cannot be run against a live server. {@code java_token_source_refresh} covers the
 * caching and the refresh behaviour this delegates, which is the part the SDK owns.
 *
 * <p>The client is a <b>public</b> OAuth client: the gateway exchanges the token, so a client
 * secret is never involved (D447/D449). That is why there is no secret field to leak into a log
 * or a config file.
 */
public final class OidcExchange implements TokenSource {

    /** The RFC 8693 grant type for exchanging one token for another. */
    public static final String GRANT_TYPE = "urn:ietf:params:oauth:grant-type:token-exchange";

    /** The subject token type this exchange sends: an OIDC id token. */
    public static final String SUBJECT_TOKEN_TYPE = "urn:ietf:params:oauth:token-type:id_token";

    /** The requested token type: an OAuth access token. */
    public static final String REQUESTED_TOKEN_TYPE = "urn:ietf:params:oauth:token-type:access_token";

    private final String endpoint;
    private final String clientId;
    private final Supplier<String> subjectToken;
    private final HttpClient http;
    private final RefreshingTokenSource inner;

    /**
     * @param endpoint the instance's {@code /oauth/token} endpoint
     * @param clientId the public OAuth client id
     * @param subjectToken mints the current identity token, from the host's OIDC session
     * @param http the client that posts the form body; {@code null} builds one
     */
    public OidcExchange(
            String endpoint, String clientId, Supplier<String> subjectToken, HttpClient http) {
        this.endpoint = endpoint;
        this.clientId = clientId;
        this.subjectToken = subjectToken;
        this.http =
                http != null
                        ? http
                        : HttpClient.newBuilder()
                                .version(HttpClient.Version.HTTP_2)
                                .connectTimeout(java.time.Duration.ofSeconds(20))
                                .build();
        this.inner = new RefreshingTokenSource(this::exchange);
    }

    /** The cached Loams access token, exchanging one first if the cache is empty. */
    @Override
    public String token() {
        return inner.token();
    }

    /** Exchange a new access token. */
    @Override
    public void refresh() {
        inner.refresh();
    }

    /**
     * Post the RFC 8693 token exchange and return the access token.
     *
     * <p>The form is the documented one: grant type, both token types, the subject token, the
     * client id and the audience. There is no {@code client_secret} parameter, and that is not an
     * omission.
     */
    private String exchange() {
        if (subjectToken == null) {
            throw Errors.internal(
                    "", "the OIDC token source has no subjectToken supplier", null);
        }
        String subject = subjectToken.get();
        if (subject == null || subject.isEmpty()) {
            throw Errors.internal(
                    "", "the OIDC token source's subjectToken supplier returned nothing", null);
        }
        Map<String, String> form = new LinkedHashMap<>();
        form.put("grant_type", GRANT_TYPE);
        form.put("subject_token_type", SUBJECT_TOKEN_TYPE);
        form.put("requested_token_type", REQUESTED_TOKEN_TYPE);
        form.put("subject_token", subject);
        form.put("client_id", clientId);
        form.put("audience", endpoint);

        HttpRequest request =
                HttpRequest.newBuilder(URI.create(endpoint))
                        .header("Content-Type", "application/x-www-form-urlencoded")
                        .header("Accept", "application/json")
                        .POST(HttpRequest.BodyPublishers.ofString(formEncoded(form)))
                        .build();
        HttpResponse<String> response;
        try {
            response = http.send(request, HttpResponse.BodyHandlers.ofString(StandardCharsets.UTF_8));
        } catch (IOException e) {
            throw Errors.internal("", "the OIDC token exchange did not complete: " + e, e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw Errors.internal("", "the OIDC token exchange was interrupted", e);
        }
        if (response.statusCode() / 100 != 2) {
            throw Errors.internal(
                    "",
                    "the token exchange answered " + response.statusCode() + ": " + response.body(),
                    null);
        }
        try {
            String accessToken =
                    Json.string(Json.parseObject(response.body()), "access_token");
            if (accessToken == null || accessToken.isEmpty()) {
                throw Errors.internal(
                        "", "the token exchange answered no access_token", null);
            }
            return accessToken;
        } catch (IllegalArgumentException e) {
            throw Errors.internal("", "the token exchange's answer was not JSON", e);
        }
    }

    /** {@code application/x-www-form-urlencoded}, with the RFC 6749 §2.3.1 escaping. */
    static String formEncoded(Map<String, String> form) {
        StringBuilder out = new StringBuilder();
        for (Map.Entry<String, String> entry : form.entrySet()) {
            if (out.length() > 0) {
                out.append('&');
            }
            out.append(urlEncode(entry.getKey())).append('=').append(urlEncode(entry.getValue()));
        }
        return out.toString();
    }

    private static String urlEncode(String value) {
        if (value == null) {
            return "";
        }
        StringBuilder out = new StringBuilder(value.length());
        for (byte b : value.getBytes(StandardCharsets.UTF_8)) {
            int c = b & 0xff;
            boolean unreserved =
                    (c >= 'A' && c <= 'Z')
                            || (c >= 'a' && c <= 'z')
                            || (c >= '0' && c <= '9')
                            || c == '-'
                            || c == '.'
                            || c == '_'
                            || c == '~';
            if (unreserved) {
                out.append((char) c);
            } else {
                out.append('%').append(String.format("%02X", c));
            }
        }
        return out.toString();
    }

    @Override
    public String toString() {
        return "oidc(" + clientId + ")";
    }
}