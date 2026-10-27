//! The vendored Spark Connect protos keep the shape the proxy relies on
//! (D1994): every request message has `string session_id = 1` and
//! `UserContext user_context = 2`.

use loams_spark::proto::{self, FILE_DESCRIPTOR_SET, SERVICE};
use prost::Message as _;
use prost_types::field_descriptor_proto::{Label, Type};

#[test]
fn method_list_matches_the_descriptor() {
    let methods: Vec<String> = proto::service_methods(SERVICE)
        .into_iter()
        .map(|(m, _)| m)
        .collect();
    assert_eq!(methods, proto::METHODS);
}

#[test]
fn every_request_starts_with_session_id_and_user_context() {
    let set = prost_types::FileDescriptorSet::decode(FILE_DESCRIPTOR_SET).unwrap();
    let messages: std::collections::HashMap<String, &prost_types::DescriptorProto> = set
        .file
        .iter()
        .flat_map(|f| {
            f.message_type
                .iter()
                .map(move |m| (format!(".{}.{}", f.package(), m.name()), m))
        })
        .collect();
    let methods = proto::service_methods(SERVICE);
    assert!(!methods.is_empty());
    for (method, input) in methods {
        let msg = messages
            .get(&input)
            .unwrap_or_else(|| panic!("{method}: input {input} not found"));
        let field = |n: i32| msg.field.iter().find(|f| f.number() == n);
        let sid = field(1).unwrap_or_else(|| panic!("{method}: no field 1"));
        assert_eq!(sid.name(), "session_id", "{method}");
        assert_eq!(sid.r#type(), Type::String, "{method}");
        assert_ne!(sid.label(), Label::Repeated, "{method}");
        let ctx = field(2).unwrap_or_else(|| panic!("{method}: no field 2"));
        assert_eq!(ctx.name(), "user_context", "{method}");
        assert_eq!(ctx.type_name(), ".spark.connect.UserContext", "{method}");
    }
}

#[test]
fn user_context_user_id_is_field_one() {
    let set = prost_types::FileDescriptorSet::decode(FILE_DESCRIPTOR_SET).unwrap();
    let uc = set
        .file
        .iter()
        .flat_map(|f| f.message_type.iter())
        .find(|m| m.name() == "UserContext")
        .unwrap();
    let f = uc.field.iter().find(|f| f.number() == 1).unwrap();
    assert_eq!(f.name(), "user_id");
    assert_eq!(f.r#type(), Type::String);
}

#[test]
fn pinned_tag() {
    assert_eq!(proto::SPARK_TAG, "v4.1.3");
}
