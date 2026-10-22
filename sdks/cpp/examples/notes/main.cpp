// A notes app on Loams: a namespace, a collection with a schema, documents, a
// hybrid search with a filter, and a graph query.
//
//   LOAMS_ENDPOINT=http://127.0.0.1:8080 LOAMS_NAMESPACE=app ./build/notes

#include <iostream>

#include <loams/loams.hpp>

int main() {
  try {
    loams::Options options = loams::OptionsFromEnv();
    options.session_consistency = true;  // reads see this client's writes
    const std::string ns = options.default_namespace;
    std::shared_ptr<loams::Loams> client = loams::MakeLoams(std::move(options));

    try {
      client->Namespaces()->Create(ns);
    } catch (const loams::AlreadyExistsError&) {
      // Created by an earlier run.
    }
    try {
      client->Collections()->Create(
          "notes", loams::Schema().Keyword("title").Text("body").Keyword("tags").I64("n").Vector("embedding", 4));
    } catch (const loams::AlreadyExistsError&) {
    }

    loams::CollectionHandle notes = client->Collection("notes");
    notes.Upsert({
        loams::Document::FromJson(1, R"({"title":"Refunds","body":"refund policy for orders","tags":["policy"],"n":1})",
                                  {{"embedding", {1, 0, 0, 0}}}),
        loams::Document::FromJson(2, R"({"title":"Shipping","body":"shipping times","tags":["ops"],"n":2})",
                                  {{"embedding", {0, 1, 0, 0}}}),
        loams::Document::FromJson(
            3, R"({"title":"Refund window","body":"the refund window is thirty days","tags":["policy"],"n":3})",
            {{"embedding", {0.9f, 0.1f, 0, 0}}}),
    });

    loams::SelectOptions title;
    title.include = {"title"};
    // k is explicit so the example also runs on a 0.1.0 engine, which has no default k.
    loams::VectorOptions ten;
    ten.k = 10;
    const auto hits = notes.Search(loams::Search()
                                       .Text("refund", {{"body"}, 10})
                                       .Vector("embedding", {1, 0, 0, 0}, ten)
                                       .Fuse(loams::Fusion::Rrf(60))
                                       .Where(loams::Filter::Eq("tags", loams::Json("policy")))
                                       .Highlight("body")
                                       .Select(title)
                                       .Limit(5));
    for (const auto& hit : hits.hits()) {
      std::cout << loams::DocId::From(hit.pk()).Text() << ": " << hit.source().fields().at("title").string_value()
                << " (" << hit.score() << ")\n";
    }
    std::cout << notes.Count().count() << " notes\n";

    // Graph is experimental and not in every build.
    try {
      client->Graph()->Create("notes_graph");
      client->Graph()->Query("notes_graph", "INSERT (:Note {id: $id})-[:TAGGED]->(:Tag {name: $tag})",
                             {{"id", loams::GraphValue(1)}, {"tag", loams::GraphValue("policy")}});
      loams::GraphQueryOptions read;
      read.read_only = true;
      const auto rows = client->Graph()->Query(
          "notes_graph", "MATCH (n:Note)-[:TAGGED]->(t:Tag) RETURN n.id AS id, t.name AS tag", {}, read);
      for (const auto& record : loams::GraphModule::Records(rows)) {
        std::cout << "note " << record.at("id").number_value() << " is tagged " << record.at("tag").string_value()
                  << "\n";
      }
      client->Graph()->Delete("notes_graph");
    } catch (const loams::FeatureNotInVariantError&) {
      std::cout << "this server has no graph\n";
    }
    return 0;
  } catch (const std::exception& error) {
    std::cerr << error.what() << "\n";
    return 1;
  }
}
