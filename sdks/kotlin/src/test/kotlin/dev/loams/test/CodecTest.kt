package dev.loams.test

import dev.loams.CallBinding
import dev.loams.ModuleBinding
import dev.loams.Streaming
import dev.loams.test.Support.ConformanceReport
import dev.loams.test.Support.CorpusDriver
import dev.loams.test.Support.FixtureServer
import dev.loams.test.Support.RepositoryRoot

/**
 * The runtime-contract tests about the SDK's **shape** rather than about a call:
 * that the facade is derived from the committed descriptor set, that the descriptor
 * set is the only source of truth about the API, and that the report is generated.
 *
 * These are the tests that would catch the failure mode the whole design is aimed
 * at: a binding table transcribed by hand that stops matching the protos while
 * still passing the corpus, because the corpus only touches 28 RPCs out of 25.
 */
object CodecTest {
    fun register() {
        Harness.test("kotlin_the_facade_is_derived_from_the_descriptor_set") {
            // **Derived, never transcribed.** The modules and their calls come out
            // of the `loams.options.v1.module` and `loams.options.v1.facade`
            // // options on the committed descriptor set, not out of a table in this
            // repository. A proto that adds a module is in the next build with nobody
            // editing this file.
            val modules = dev.loams.Facade.modules
            assertTrue(modules.isNotEmpty(), "modules derived from the descriptor set")
            assertEquals(
                listOf("instance", "live"),
                modules.map { it.name }.sorted(),
                "the two services that carry a loams.options.v1.module annotation",
            )

            val instance = modules.first { it.name == "instance" }
            assertEquals("loams.instance.v1.InstanceService", instance.service, "InstanceService's binding")
            assertTrue(!instance.unstable, "instance to be a stable module")

            // R1's rule: a package whose wire contract may still change is marked,
            // so a caller feature-detects rather than depends on it.
            val live = modules.first { it.name == "live" }
            assertTrue(live.unstable, "loams.live.v1 to be marked unstable (R1)")

            // The calls on them, with the names the annotations gave.
            assertEquals(
                listOf("getInstance", "whoAmI"),
                dev.loams.Facade.module("instance").calls.map { it.name }.sorted(),
                "the instance module's calls",
            )
            assertEquals(
                listOf("modifyQuerySet", "watch"),
                dev.loams.Facade.module("live").calls.map { it.name }.sorted(),
                "the live module's calls",
            )
            // `FacadeOptions.module = "tables"` puts query/mutate/deploy on a
            // *different* module than the service's own — §44 §7.2 splits
            // `loams.live.v1` into the session half and the table half.
            assertEquals(
                listOf("query", "mutate", "deploy"),
                dev.loams.Facade.module("tables").calls.map { it.name }.sorted(),
                "the tables module's calls, from FacadeOptions.module",
            )
        }

        Harness.test("kotlin_every_rpc_in_the_descriptor_set_is_reachable_by_path") {
            // The generic path the corpus driver uses: a binding resolved from an RPC
            // path, for **every** service in the set — not only the two that carry a
            // module annotation. This is what lets the 13 `mock_*` fixtures run with
            // no facade at all.
            val services = dev.loams.Descriptors.services
            assertTrue(services.size >= 5, "the services in the descriptor set (${services.map { it.fullName }})")
            var rpcs = 0
            for (service in services) {
                for (method in service.methods) {
                    val rpc = "${service.fullName}/${method.name}"
                    val binding = dev.loams.Descriptors.bindingForRpc(rpc)
                    assertTrue(binding != null, "a binding for $rpc")
                    binding!!
                    assertEquals(rpc, binding.rpc, "$rpc's own path")
                    assertEquals(
                        if (method.isServerStreaming) Streaming.SERVER else Streaming.UNARY,
                        binding.streaming,
                        "$rpc's streaming, asked of the service descriptor",
                    )
                    rpcs++
                }
            }
            assertTrue(rpcs >= 25, "every RPC in the set to be reachable ($rpcs)")

            // And the two the corpus has no facade for are among them, which is the
            // whole point of the generic path.
            for (rpc in listOf(
                "loams.approvals.v1.ApprovalService/DecideApproval",
                "loams.devices.v1.DeviceService/SendTestNotification",
                "loams.approvals.v1.ApprovalService/WatchApprovals",
            )) {
                val binding = dev.loams.Descriptors.bindingForRpc(rpc)
                assertTrue(binding != null, "a binding for $rpc")
                assertTrue(
                    binding!!.module == null,
                    "$rpc to carry no module, because ApprovalService and DeviceService carry no annotation",
                )
            }
        }

        Harness.test("kotlin_a_binding_carries_its_request_and_response_types") {
            val binding = requireNotNull(
                dev.loams.Descriptors.bindingForRpc("loams.instance.v1.InstanceService/GetInstance")
            )
            assertEquals("loams.instance.v1.GetInstanceRequest", binding.request.fullName, "the request type")
            assertEquals("loams.instance.v1.GetInstanceResponse", binding.response.fullName, "the response type")
            // And the descriptor set is the *only* source: asking for a message the
            // protos do not declare returns null rather than an empty descriptor.
            assertTrue(dev.loams.Descriptors.message("loams.instance.v1.NoSuchMessage") == null, "a message that does not exist")
        }

        Harness.test("kotlin_the_retry_class_is_derived_from_the_proto_not_written_down") {
            // D610: the class comes from `idempotency_level` and from whether the
            // request's schema declares an `idempotency_key`. A binding that claimed
            // a mutation is safe would be a binding the annotations did not say so.
            val decide = requireNotNull(
                dev.loams.Descriptors.bindingForRpc("loams.approvals.v1.ApprovalService/DecideApproval")
            )
            assertTrue(decide.takesIdempotencyKey, "DecideApprovalRequest to declare a key field")
            assertEquals(dev.loams.RetryClass.MANUAL, decide.retry, "a mutation's class before keying")

            val list = requireNotNull(
                dev.loams.Descriptors.bindingForRpc("loams.approvals.v1.ApprovalService/ListApprovals")
            )
            assertTrue(!list.takesIdempotencyKey, "ListApprovalsRequest to declare no key field")
            assertEquals(dev.loams.RetryClass.SAFE, list.retry, "NO_SIDE_EFFECTS' class")
        }

        Harness.test("kotlin_the_six_conformance_test_names_exist") {
            testNamesExist()
            // Asserted here as well as in its own test, so a run of just this test
            // still proves the contract.
            val registered = Harness.names()
            for (name in ConformanceReport.REQUIRED_TEST_NAMES) {
                assertTrue(registered.any { it.contains(name) }, "a registered test named $name")
            }
        }

        Harness.test("kotlin_the_report_is_generated_not_committed") {
            reportIsGeneratedNotCommitted()
        }

        Harness.test("kotlin_the_required_fixture_list_is_derived_from_the_manifest") {
            coverageIsDerived()
        }

        Harness.test("kotlin_the_fixture_server_is_reachable") {
            fixtureServerComesUp()
        }

        Harness.test("kotlin_a_client_requires_an_endpoint") {
            clientRequiresAnEndpoint()
        }

        Harness.test("kotlin_the_json_parser_refuses_what_it_does_not_understand") {
            jsonParserRefusesGarbage()
        }

        Harness.test("kotlin_the_envelope_reader_reports_a_truncated_stream") {
            envelopeReaderReportsATruncatedStream()
        }

        Harness.test("kotlin_the_content_types_are_the_four_the_corpus_records") {
            contentTypesAreTheFourTheCorpusRecords()
        }

        Harness.test("kotlin_the_codec_round_trips_through_both_encodings") {
            codecRoundTrips()
        }

        Harness.test("kotlin_proto3_json_is_compact_and_field_ordered") {
            proto3JsonIsCompactAndFieldOrdered()
        }

        Harness.test("kotlin_compact_json_refuses_a_well_known_type") {
            compactJsonRefusesAWellKnownType()
        }

        Harness.test("kotlin_a_refusal_is_not_an_http_status") {
            aRefusalIsNotAnHttpStatus()
        }

        Harness.test("kotlin_the_error_detail_decoder_is_tolerant_of_padding_and_strict_of_alphabet") {
            base64IsTolerantOfPaddingAndStrictOfAlphabet()
        }

        Harness.test("kotlin_the_error_envelope_codes_match_the_registry") {
            errorEnvelopeCodesMatchTheRegistry()
        }

        Harness.test("kotlin_every_registry_reason_has_a_class") {
            everyRegistryReasonHasAClass()
        }

        Harness.test("kotlin_idempotency_is_decided_from_the_schema") {
            idempotencyIsDecidedFromTheSchema()
        }

        Harness.test("kotlin_uuid_v7_sorts_by_time") {
            uuidV7SortsByTime()
        }

        Harness.test("kotlin_the_retry_backoff_is_capped_and_jittered") {
            retryBackoffIsCappedAndJittered()
        }

        Harness.test("kotlin_the_retry_classes_are_the_ones_d610_names") {
            retryClassesAreTheOnesD610Names()
        }

        Harness.test("kotlin_the_retry_class_comes_from_the_proto") {
            retryClassComesFromTheProto()
        }

        Harness.test("kotlin_a_cancelled_call_spends_no_attempt") {
            cancelledCallSpendsNoAttempt()
        }

        Harness.test("kotlin_the_reason_registry_matches_the_registry_page") {
            reasonRegistryMatchesTheRegistryPage()
        }

        Harness.test("kotlin_a_stream_sends_a_framed_request") {
            streamRequestsAreFramed()
        }

        Harness.test("kotlin_a_refreshing_token_source_mints_once") {
            refreshingTokenSourceMintsOnce()
        }

        Harness.test("kotlin_a_static_token_source_cannot_refresh") {
            staticTokenSourceCannotRefresh()
        }

        Harness.test("kotlin_an_empty_token_sends_no_Authorization") {
            emptyTokenSourceSendsNoAuthorization()
        }

        Harness.test("kotlin_the_bearer_is_not_attached_when_there_is_no_source") {
            noSourceSendsNoAuthorization()
        }

        Harness.test("kotlin_the_token_source_is_read_once_per_attempt") {
            tokenSourceReadCounts()
        }

        Harness.test("kotlin_the_bearer_survives_a_caller_header_in_a_different_case") {
            bearerHeaderIsCaseInsensitive()
        }

        Harness.test("kotlin_a_client_without_a_token_source_still_sends_the_callers_headers") {
            callersHeadersSurvive()
        }

        Harness.test("kotlin_a_transport_failure_is_typed") {
            transportFailureIsTyped()
        }

        Harness.test("kotlin_a_unary_request_body_is_not_framed") {
            unaryBodyIsUnframed()
        }

        Harness.test("kotlin_the_pagination_iterator_sends_one_call_per_page") {
            paginationSendsOneCallPerPage()
        }

        Harness.test("kotlin_pagination_refuses_an_unknown_items_field") {
            paginationRefusesAnUnknownItemsField()
        }

        Harness.test("kotlin_a_repeated_token_does_not_re_ask_forever") {
            aRepeatedTokenTerminates()
        }
    }
}

/**
 * `kotlin_the_driver_derives_its_coverage_from_the_manifest` — the anti-vacuity
 * property, asserted about the **driver** rather than about the SDK.
 *
 * Two directions, because a driver that is merely wrong in one of them would look
 * fine: a fixture the manifest newly marks `required` is in the next run with
 * nobody editing the driver, and a fixture the driver cannot reach is **named in the
 * failure** rather than dropped from a hand-written list.
 */
fun theDriverDerivesItsCoverage() {
    Harness.test("kotlin_the_driver_derives_its_coverage_from_the_manifest") {
        val root = RepositoryRoot.find(java.io.File(".").absoluteFile)
        val driver = CorpusDriver(java.io.File(root, "sdks/fixtures"))

        val required = driver.requiredFixtures
        val manifest = dev.loams.Json.parse(
            java.io.File(root, "sdks/fixtures/manifest.json").readText()
        ).asObject()!!
        val manifestRequired = manifest["fixtures"]!!.asArray()!!
            .mapNotNull { it.asObject() }
            .filter { it["required"]?.asBoolean() == true }
            .mapNotNull { it.string("name") }

        // Exactly the manifest's required set: not a subset, not a superset.
        assertEquals(manifestRequired.size, required.size, "required fixtures counted")
        assertEquals(manifestRequired.sorted(), required.map { it.name }.sorted(), "required fixtures")

        // Every one names a file that exists. A manifest entry with no file behind it
        // is a corpus bug, and this says so by name rather than as a missing-file
        // error pointing at a path nobody wrote.
        for (fixture in required) {
            val file = java.io.File(root, "sdks/fixtures").resolve(fixture.file)
            assertTrue(file.isFile, "${fixture.name}'s recording at ${fixture.file}")
        }

        // And the clauses are read from `pinnedBy`, so the report can name them.
        val withClauses = required.filter { it.clauses.isNotEmpty() }
        assertTrue(
            withClauses.size == required.size,
            "every required fixture to pin at least one clause (${required.filter { it.clauses.isEmpty() }.map { it.name }})",
        )
    }
}

/**
 * `kotlin_a_fixture_the_driver_cannot_reach_is_named` — the negative case, which a
 * green run never exercises on its own.
 *
 * A driver that silently dropped an unreachable fixture would report 28 of 28 and
 * be wrong. This builds a driver over a corpus directory with a **mangled** copy of
 * one recording and asserts the failure names the fixture, so the property is
 * demonstrated rather than asserted.
 */
fun anUnreachableFixtureIsNamed() {
    Harness.test("kotlin_a_fixture_the_driver_cannot_reach_is_named") {
        val root = RepositoryRoot.find(java.io.File(".").absoluteFile)
        val source = java.io.File(root, "sdks/fixtures")
        val scratch = java.io.File.createTempFile("loams-corpus", "")
        scratch.delete()
        scratch.mkdirs()

        // Copy the corpus, then break exactly one recording's path so its RPC
        // resolves to nothing.
        copyTree(source, scratch)
        val broken = java.io.File(scratch, "recorded/instance_who_am_i_json.json")
        val original = broken.readText()
        broken.writeText(original.replace("InstanceService/WhoAmI", "InstanceService/WhoAmINotThere"))

        val driver = CorpusDriver(scratch)
        val outcomes = driver.run().outcomes
        val brokenOutcome = outcomes.firstOrNull { it.name == "instance_who_am_i_json" }
        assertTrue(brokenOutcome != null, "an outcome for the broken fixture")
        assertTrue(!brokenOutcome!!.ran, "the broken fixture to be reported as not run")
        assertTrue(
            brokenOutcome.failures.any { it.contains("instance_who_am_i_json") },
            "the failure to name the fixture (said: ${brokenOutcome.failures})",
        )
        scratch.deleteRecursively()
    }
}

/** Recursively copies a directory, for the negative case above. */
private fun copyTree(from: java.io.File, to: java.io.File) {
    from.walkTopDown().forEach { source ->
        val target = java.io.File(to, source.relativeTo(from).path)
        if (source.isDirectory) {
            target.mkdirs()
        } else {
            target.parentFile.mkdirs()
            source.copyTo(target, overwrite = true)
        }
    }
}

/** `kotlin_the_report_is_written_from_what_the_driver_executed`. */
fun theReportIsWrittenFromWhatTheDriverExecuted() {
    Harness.test("kotlin_the_report_is_written_from_what_the_driver_executed") {
        val root = RepositoryRoot.find(java.io.File(".").absoluteFile)
        val report = java.io.File(root, ConformanceReport.REPORT_PATH)
        assertTrue(report.isFile, "a report at ${ConformanceReport.REPORT_PATH}")

        val written = dev.loams.Json.parse(report.readText()).asObject()!!
        assertEquals("kotlin", written.string("language"), "the report's language")
        assertEquals("connect", written.string("transport"), "the report's transport")
        assertTrue(
            !written.boolean("live"),
            "the report to say it was a replay (this SDK never exercises a real loams dev)",
        )
        assertEquals(
            ConformanceReport.REQUIRED_TEST_NAMES,
            written["tests"]!!.asArray()!!.map { it.asString() },
            "the report's six test names",
        )
        // `skipped` is empty by construction: `required.mjs` permits exactly one
        // skip, and this SDK is not the Connect-unary fallback D613 names.
        assertEquals(emptyList<String>(), written["skipped"]!!.asArray()!!.map { it.asString() }, "the report's skipped list")
    }
}

/** A binding the driver would use, for a caller of this file. */
fun bindingFor(rpc: String): CallBinding = requireNotNull(dev.loams.Descriptors.bindingForRpc(rpc)) {
    "the committed descriptor set declares no RPC '$rpc'"
}

/** A module the facade derived, for a caller of this file. */
fun moduleFor(name: String): ModuleBinding = dev.loams.Facade.module(name)

/** A guard that a module that does not exist says so with the ones that do. */
fun aMissingModuleNamesTheOnesThatExist() {
    Harness.test("kotlin_a_missing_module_names_the_ones_that_exist") {
        val error = assertThrows<NoSuchElementException>("a module that does not exist") {
            dev.loams.Facade.module("collections")
        }
        assertTrue(
            error.message!!.contains("instance"),
            "the failure to name the modules that exist (said: ${error.message})",
        )
    }
}

/** A guard that the fixture server's report endpoint carries the manifest. */
fun theFixtureServerServesTheManifest() {
    Harness.test("kotlin_the_fixture_server_serves_the_manifest") {
        val root = RepositoryRoot.find(java.io.File(".").absoluteFile)
        FixtureServer.start(java.io.File(root, "sdks/fixtures")).use { server ->
            val connection = java.net.URI("${server.endpoint}/__fixtures/manifest").toURL().openConnection()
            connection.connectTimeout = 5_000
            connection.readTimeout = 5_000
            val text = connection.getInputStream().bufferedReader().readText()
            val manifest = dev.loams.Json.parse(text).asObject()
            assertTrue(manifest != null, "the manifest over HTTP")
            assertTrue(
                manifest!!["fixtures"]!!.asArray()!!.isNotEmpty(),
                "the fixtures the server would answer for",
            )
        }
    }
}