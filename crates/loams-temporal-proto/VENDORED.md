# Vendored: `temporalio/api` v1.63.5

The protos under `proto/temporal/`, `proto/google/api/` and
`proto/nexusannotations/` are copied **unchanged** from
[`temporalio/api`](https://github.com/temporalio/api) (MIT, "Copyright (c)
2025 Temporal Technologies Inc."; the full text is `LICENSE-temporalio-api`).
`proto/loams/` is Loams' own (Apache-2.0).

| | |
|---|---|
| Tag | `v1.63.5` (plan DX1 ruling X1.R1: the `go.temporal.io/api` version Temporal server v1.32.1 and the Go SDK v1.49.0 both pin) |
| Commit | `3ebdff42a9f07ac484b415fe8ff0b483b4ce3340` (2026-08-04) |
| Source | `https://codeload.github.com/temporalio/api/tar.gz/refs/tags/v1.63.5` |
| Tarball SHA-256 | `359fd496ee2d97ed9776bae473e3d65c9c037b5370af8c1f2e9008043c1136a8` |
| Taken | `temporal/**` (every file), `google/api/{annotations,http}.proto` and `nexusannotations/v1/options.proto` (the only imports outside `temporal/` besides protobuf's well-known types, which `protoc` provides) |

## Updating

1. Pick the tag the pinned server and SDKs use (`go.temporal.io/api` in their
   `go.mod`), at least 14 days old (D1274).
2. Download the tag's tarball, check its SHA-256, and replace the three
   subtrees above with the tarball's copies. Change nothing inside them.
3. Regenerate the list below with
   `(cd proto && find temporal google nexusannotations -type f | LC_ALL=C sort | xargs sha256sum)`
   and update this table, `API_TAG` and `API_COMMIT` in `src/lib.rs`, and
   the method count in `tests/proto.rs`.
4. Run `buf breaking` against the previous tag and record any break in the
   plan's rulings.

## Files

The test `vendored_files_match_manifest` checks this list against `proto/`.

```
d3d3b4bbe80c0481c35507376f5653fcfdfcbdc7f168aab097f25ae643b7ebe8  google/api/annotations.proto
2ffe191f209a9a68e30f0493c66d5ce15be8deac276d55bcd4127f8cc7f453e2  google/api/http.proto
9553d399a8e013bf2a360942356165460100b7f8dc16aaf35964ca8c9f7470e8  nexusannotations/v1/options.proto
84ed6738c43c2686ba9e69052f1b587cf649b1fe01c4a86f62a85687b2e04308  temporal/api/activity/v1/message.proto
b8270d4d7e24d7756076c95d8231f0934196d603eda741f7f56ebd426f12cc28  temporal/api/batch/v1/message.proto
b213df854201b3b15e230e792e0730a88f77de73881066b9da25f8e6f314fca2  temporal/api/callback/v1/message.proto
eab8318bcde26bf0c356643e7bf222c39fc243791941e588f6c20a810453cb30  temporal/api/command/v1/message.proto
e263403e92b4bb7d1f4b0b05d077c0b54da8a0439df34afe8c3d9aa4147df69f  temporal/api/common/v1/message.proto
7e6e49963f98d1885bd9627fdecf8f09fcfd285c67dcf4561731a45dd1c5f24d  temporal/api/compute/v1/config.proto
70445932d1f2448edf69f052d85aa61e5ee109534cc84ae4a51b52545eed0959  temporal/api/compute/v1/provider.proto
ecdbc8c650882fcd9bde816f728724786b5ea448e7dc5ee440f9f0d8c099eecf  temporal/api/compute/v1/scaler.proto
9b811db7e70ca688d15bfd6b9660e69830cd3f714c821427a025097ed7205c0c  temporal/api/deployment/v1/message.proto
4f23dba5731649048a8508eaf0ff594b67b2f7b053044af15c04c92d531f7e0a  temporal/api/enums/v1/activity.proto
50d1b6fcabf082421841f5a144a19f829f2ecd1f72f395070c6d5c490b105fb9  temporal/api/enums/v1/batch_operation.proto
735c758ab126c02eee5f7da3666a0524ef17424ef23ddfaa3b54e9c687acef25  temporal/api/enums/v1/command_type.proto
0b44e48de5233c7799e1b48829d21cc08bed778d99ad3493504c5eba1cccbb06  temporal/api/enums/v1/common.proto
92d1d1b1b0e780932cebcbee7ec19f7fe13d5701b5ab77ad2c80fa35b9b5af26  temporal/api/enums/v1/deployment.proto
f0bc36625acfa15daa79fbf80969736c2c28430d08318fc248d56152e6ac9429  temporal/api/enums/v1/event_type.proto
82160a971b24665dae513b1f2301e9c7b37ca044dc75877543d4f042587708b7  temporal/api/enums/v1/failed_cause.proto
a66d8f43cfb968b3c11b96f900a24d0f964a5e3c1ae469c197c43830bae50760  temporal/api/enums/v1/namespace.proto
e6cde46beb51624fc6839f40f7aae15a15d684df7e8f26ac93df019f73a1511e  temporal/api/enums/v1/nexus.proto
efd50192850571d9e59dd986cc86d35defd7e97bf2a01e7fc0776b43e5379a91  temporal/api/enums/v1/query.proto
e4f147334843dfe21fecc18d9a02d96ab2465b754d4c19e04ee3164b85f06ad7  temporal/api/enums/v1/reset.proto
fefd22734611ee1e8319413630765db2120acc9336b34b2e6586b046c6cd3c8e  temporal/api/enums/v1/schedule.proto
e2cb0acbde0f0f4bd272642a49802da98f8aa72ac614bd67dc0e9b462063998e  temporal/api/enums/v1/task_queue.proto
e8f9d311132016a7a19cd02bf933354612af395ed6fb0a80cd5f9379ca84243e  temporal/api/enums/v1/time_skipping.proto
963fd235a61b54110e9f37060a1700109ec2b97c92ff7bed38e261fbb80c9bf5  temporal/api/enums/v1/update.proto
cd2cc13a066609452afe2eaf47e63ebfca2e3c0628c1dbd6790b2fa70d3f65f8  temporal/api/enums/v1/workflow.proto
6997df1aa8284e1613370f2c43da280aca23c59696ffc9be0acdf8c2d6507f42  temporal/api/errordetails/v1/message.proto
5e30f5b017c49ca3af72afabbeaa480d1f654ca88f5fea863bbd8b2bc7349154  temporal/api/export/v1/message.proto
00ee339621c80d1dd501d58e4db4ecd666932c49cb6ab15804f17bb0181ca18f  temporal/api/failure/v1/message.proto
7116b319287254ad338c71d926569d9bd603d0f8ab0f1a9c8124671f55c293c5  temporal/api/filter/v1/message.proto
86f14f402ec466f99fc17ef0e84e06350b0817dce9404619c5949ef6b69d35f1  temporal/api/history/v1/message.proto
dd595445ee997f98ccb7febb98b72edb22067a81d6cd2f0ca106868d800e0030  temporal/api/namespace/v1/message.proto
b3ff7a433b6eed36c7d54fcc4d9e657928b1c8dbb113abeb1e62d5a9bf383c1e  temporal/api/nexus/v1/message.proto
42161d0954a40ec8b58042dfa3166e42b24c65af6d658cd0a97b81dc874012cd  temporal/api/nexusservices/workerservice/v1/request_response.proto
2ffb88e65fd656d603037f1fbd34ad50561cbc6a07155a762b22d986297ac130  temporal/api/operatorservice/v1/request_response.proto
2c2e8ebcdbd5adf058edfff247d75a1fc491d1f551efa72506b827cb71353eb4  temporal/api/operatorservice/v1/service.proto
59d33cbdfe9869d349d39a1e27d5b633251e8be187e6da29a19e98708602f9cb  temporal/api/protocol/v1/message.proto
54e36a4f3b69af5f9aadeff72f5404383b1357de2f798047ae947cb8d6edc332  temporal/api/protometa/v1/annotations.proto
6251887f60ec7db05b6e0c26d924cda7ff971da84a667b52fe5eeb89a4b60a6e  temporal/api/query/v1/message.proto
45a653d718404954d389236fa80cb00530e33a8944211d050fe1ec5f5fc618ac  temporal/api/replication/v1/message.proto
67e785b2dbf26e59715a95ec439ebf30840d833f3e38d301ce0d6bf2618d0e65  temporal/api/rules/v1/message.proto
666b4e2efc16640b0ee57b2c068bc5135228794519c6e77aba617ff62ff8ed0f  temporal/api/schedule/v1/message.proto
407669aa6a3a3569012bf9ca4727b3f18a42032234d87e2694795967162a54ff  temporal/api/sdk/v1/enhanced_stack_trace.proto
bc9e00c9cffd9af91d120bb946f9a408b6d56a4ae92a14ae8b7c9b0f8197da91  temporal/api/sdk/v1/event_group_marker.proto
a5080e3f65c11108ce552b13105a5265b64b904a3b5abb88887730341a12d2da  temporal/api/sdk/v1/external_storage.proto
14778430c3ec8e4768be26446520b02e1888455f2d54ebd199bf8c5e3d8d324b  temporal/api/sdk/v1/task_complete_metadata.proto
6cd91201312ecebd21da40685dd407cf20c6b51f2fc0a02f287013f28c13874e  temporal/api/sdk/v1/user_metadata.proto
f8d09452fedc448ef83c0bd252976fd10b25034287b73c873dadc1ff33dab75f  temporal/api/sdk/v1/worker_config.proto
71944380143bf5e0e583ea297714120265dfad18e5c957935375df34c7fd79c7  temporal/api/sdk/v1/workflow_metadata.proto
47105470fd5d1b4f900a935dfb5f0d64a8a7e8c62f52f76c77e432cb89754e05  temporal/api/taskqueue/v1/message.proto
18d30d0079d8b033ff29492ef1e4f9fe40a7bd8bf351cf8cbed76a1f2e1bc4b0  temporal/api/update/v1/message.proto
9c47ac31c13ed0429a84416973af043d69a9f095f7c0d4e7fff27e6d01384093  temporal/api/version/v1/message.proto
184f8f6e218a8ebd079b442743e616a41204ee02d5e91b06458f02818380d1d6  temporal/api/worker/v1/message.proto
3ca123d75eb8a10d6f52d868629cec689d5cd20558a025ae9c2069f804a35ebd  temporal/api/workflow/v1/message.proto
f303127eaea53bfd6a10a011ca1583443dcd7a26f6193b9a3ddb632742aef8a0  temporal/api/workflowservice/v1/request_response.proto
5a807068a6dd482f6fa0217510e33356dee4c63d79c2f166a9d4310924cce095  temporal/api/workflowservice/v1/service.proto
```
