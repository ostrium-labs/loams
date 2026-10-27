// The SDKP app scenario through the public facade, against a running server:
// `LOAMS_TEST_ENDPOINT=http://… ctest -R cpp_app_`. Without one it exits 77,
// which ctest reports as skipped. Until P0d records the scenario for the
// fixture server, this is how the app modules are checked end to end.

#include <chrono>
#include <cstdlib>
#include <map>
#include <string>
#include <vector>

#include "support.hpp"

namespace {

using namespace loams_test;  // NOLINT(build/namespaces)

std::shared_ptr<Loams> Connect(const std::string& ns) {
  Options options;
  options.endpoint = std::getenv("LOAMS_TEST_ENDPOINT");
  options.default_namespace = ns;
  options.session_consistency = true;
  if (const char* key = std::getenv("LOAMS_TEST_API_KEY")) {
    options.token_source = ApiKey(key);
  }
  return MakeLoams(std::move(options));
}

std::string FreshNamespace() {
  const auto now = std::chrono::system_clock::now().time_since_epoch();
  return "parity-cpp-" + std::to_string(std::chrono::duration_cast<std::chrono::milliseconds>(now).count()) + "-" +
         std::to_string(std::rand() % 1000);
}

Schema NotesSchema() {
  return Schema().Keyword("title").Text("body").Keyword("tags").I64("n").Vector("embedding", 4).Dynamic("ignore");
}

std::vector<Document> Notes() {
  return {
      Document::FromJson(1, R"({"title":"Refunds","body":"refund policy for orders","tags":["policy"],"n":1})",
                         {{"embedding", {1, 0, 0, 0}}}),
      Document::FromJson(2, R"({"title":"Shipping","body":"shipping times and carriers","tags":["ops"],"n":2})",
                         {{"embedding", {0, 1, 0, 0}}}),
      Document::FromJson(3, R"({"title":"Refund window","body":"the refund window is thirty days","tags":["policy"],"n":3})",
                         {{"embedding", {0.9f, 0.1f, 0, 0}}}),
      Document::FromJson(4, R"({"title":"Returns","body":"returns need a receipt","tags":["policy","ops"],"n":4})",
                         {{"embedding", {0.5f, 0.5f, 0, 0}}}),
      Document::FromJson(5, R"({"title":"Holidays","body":"holiday hours","tags":["ops"],"n":5})",
                         {{"embedding", {0, 0, 1, 0}}}),
  };
}

std::string Title(const google::protobuf::Struct& source) {
  const auto found = source.fields().find("title");
  return found == source.fields().end() ? std::string() : found->second.string_value();
}

void AppCollections() {
  const std::string ns = FreshNamespace();
  auto loams = Connect(ns);
  instance_v1::GetInstanceResponse instance;
  loams->Instance()->GetInstance(&instance);

  loams->Namespaces()->Create(ns);
  LOAMS_CHECK_THROWS(loams->Namespaces()->Create(ns), AlreadyExistsError, "a second create");
  bool listed = false;
  for (const auto& info : loams->Namespaces()->ListAll()) listed = listed || info.namespace_() == ns;
  LOAMS_CHECK(listed, "the namespace is listed");

  LOAMS_CHECK_EQ(loams->Collections()->Create("notes", NotesSchema()).name(), std::string("notes"), "create");
  LOAMS_CHECK_EQ(loams->Collections()->Get("notes").name(), std::string("notes"), "get");
  loams->Collections()->Create("scratch", Schema().Keyword("k"));
  LOAMS_CHECK_EQ(loams->Collections()->ListAll(1).size(), static_cast<std::size_t>(2), "two collections over pages");

  CallOptions keyed;
  keyed.idempotency_key = UuidV7();
  const auto first = loams->Documents()->Upsert("notes", Notes(), keyed);
  const auto again = loams->Documents()->Upsert("notes", Notes(), keyed);
  LOAMS_CHECK_EQ(first.token(), again.token(), "the same key answers the same write (R3)");

  ReadOptions read;
  read.consistency = first.token();
  const auto got = loams->Documents()->Get("notes", {1, 2}, read);
  LOAMS_REQUIRE(got.documents_size() == 2, "two documents");
  LOAMS_CHECK_EQ(Title(got.documents(0).source()), std::string("Refunds"), "the first");

  PatchChange change;
  change.set = StructFromJson(R"({"title":"Returns and exchanges"})");
  loams->Documents()->Patch("notes", 4, change);
  loams->Documents()->Delete("notes", {5});
  DocumentFilter policy;
  policy.filter = ValueFromJson(R"({"term":{"field":"tags","value":"policy"}})");
  LOAMS_CHECK_EQ(loams->Documents()->Count("notes", policy).count(), 3u, "three policy notes");
  ScrollOptions two;
  two.limit = 2;
  LOAMS_CHECK_EQ(loams->Documents()->ScrollAll("notes", two).size(), static_cast<std::size_t>(4), "scroll_all");
  LOAMS_CHECK_EQ(Title(loams->Documents()->Get("notes", {4}).documents(0).source()), std::string("Returns and exchanges"),
                 "the patch");

  LOAMS_CHECK_THROWS(loams->Collections()->Get("missing"), NotFoundError, "a missing collection");
  LOAMS_CHECK(loams->Collections()->Drop("scratch").dropped(), "drop");

  CollectionHandle handle = loams->Collection("notes");
  std::vector<Document> batch;
  for (int i = 10; i < 15; ++i) batch.push_back(Document::FromJson(i, "{\"n\":" + std::to_string(i) + "}"));
  BatchOptions chunks;
  chunks.chunk_rows = 2;
  const BatchResult result = handle.WriteBatch(batch, chunks);
  LOAMS_CHECK_EQ(result.chunks, static_cast<std::size_t>(3), "three chunks");
  ReadOptions after;
  after.consistency = result.token;
  LOAMS_CHECK_EQ(handle.Count({}, after).count(), 9u, "nine notes");
}

void AppSearch() {
  const std::string ns = FreshNamespace();
  auto loams = Connect(ns);
  loams->Namespaces()->Create(ns);
  loams->Collections()->Create("notes", NotesSchema());
  loams->Documents()->Upsert("notes", Notes());

  SelectOptions title;
  title.include = {"title"};
  const auto page = loams->Search()->Query(Search::On("notes")
                                               .Text("refund", {{"body"}, {}})
                                               .Where(Filter::Eq("tags", Json("policy")))
                                               .Highlight("body")
                                               .Select(title)
                                               .Limit(2)
                                               .SortBy({Sort::Score(), Sort::Pk()}));
  LOAMS_REQUIRE(page.hits_size() == 2, "two hits");
  LOAMS_CHECK_EQ(page.hits(0).source().fields_size(), 1, "select keeps the title");
  LOAMS_CHECK(page.hits(0).highlight_size() > 0, "highlight");

  const auto all = loams->Search()->QueryAll(
      Search::On("notes").Text("refund", {{"body"}, {}}).SortBy({Sort::Score(), Sort::Pk()}).Limit(1));
  LOAMS_CHECK_EQ(all.size(), static_cast<std::size_t>(2), "query_all over pages");

  SearchShorthand two;
  two.limit = 2;
  const auto near = loams->Vector()->Search("notes", {1, 0, 0, 0}, "embedding", two);
  LOAMS_REQUIRE(near.hits_size() > 0, "vector hits");
  LOAMS_CHECK_EQ(near.hits(0).pk().uint(), 1u, "the nearest note");

  for (const auto& fusion : {Fusion::Rrf(60), Fusion::Dbsf(), Fusion::Weighted({0.7f, 0.3f})}) {
    SearchShorthand hybrid;
    hybrid.fields = {"body"};
    hybrid.fusion = fusion;
    hybrid.filter = Filter::Eq("tags", Json("policy"));
    hybrid.limit = 3;
    LOAMS_CHECK(loams->Search()->Hybrid("notes", "refund", {1, 0, 0, 0}, "embedding", hybrid).hits_size() > 0,
                "hybrid " + ToJson(fusion));
  }
  SearchShorthand wrong;
  wrong.fields = {"body"};
  wrong.fusion = Fusion::Weighted({0.7f});
  LOAMS_CHECK_THROWS(loams->Search()->Hybrid("notes", "refund", {1, 0, 0, 0}, "embedding", wrong),
                     InvalidArgumentError, "one weight for two retrievers");
  LOAMS_CHECK_EQ(loams->Collection("notes").Search(Search().Text("refund", {{"body"}, {}}).Limit(5)).hits_size(), 2,
                 "the handle's search");
}

void AppGraph() {
  const std::string ns = FreshNamespace();
  auto loams = Connect(ns);
  loams->Namespaces()->Create(ns);
  try {
    loams->Graph()->Create("notes_graph");
  } catch (const FeatureNotInVariantError&) {
    std::cout << "this build serves no graph\n";
    return;
  }
  loams->Graph()->Query("notes_graph", "INSERT (:Note {id: $id, title: $title})-[:TAGGED]->(:Tag {name: $tag})",
                        {{"id", GraphValue(1)}, {"title", GraphValue("Refunds")}, {"tag", GraphValue("policy")}});
  GraphQueryOptions read;
  read.read_only = true;
  const auto rows =
      loams->Graph()->Query("notes_graph", "MATCH (n:Note)-[:TAGGED]->(t:Tag) RETURN n.title AS title, t.name AS tag",
                            {}, read);
  const auto records = GraphModule::Records(rows);
  LOAMS_REQUIRE(records.size() == 1, "one row");
  LOAMS_CHECK_EQ(records[0].at("title").string_value(), std::string("Refunds"), "the title");
  LOAMS_CHECK_EQ(records[0].at("tag").string_value(), std::string("policy"), "the tag");
  bool listed = false;
  for (const auto& graph : loams->Graph()->ListAll()) listed = listed || graph.name() == "notes_graph";
  LOAMS_CHECK(listed, "the graph is listed");
  loams->Graph()->Delete("notes_graph");
}

}  // namespace

int main(int argc, char** argv) {
  if (std::getenv("LOAMS_TEST_ENDPOINT") == nullptr) {
    std::cout << "LOAMS_TEST_ENDPOINT is not set\n";
    return 77;
  }
  const std::map<std::string, void (*)()> tests = {
      {"app_collections", AppCollections}, {"app_search", AppSearch}, {"app_graph", AppGraph}};
  for (int i = 1; i < argc || argc == 1; ++i) {
    for (const auto& test : tests) {
      if (argc == 1 || test.first == argv[i]) {
        try {
          test.second();
        } catch (const std::exception& error) {
          Record(__FILE__, __LINE__, test.first + " threw: " + error.what());
        }
      }
    }
    if (argc == 1) break;
  }
  return Finish("cpp_app_live");
}
