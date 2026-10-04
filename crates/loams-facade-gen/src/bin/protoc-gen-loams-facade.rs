//! `protoc-gen-loams-facade`: the buf `local` plugin of design §44 §7.3 and
//! SDK1 Task 3.
//!
//! `buf generate` starts this binary and writes a `CodeGeneratorRequest` to
//! its standard input; the plugin answers with a `CodeGeneratorResponse`. The
//! request names the files to generate and the plugin parameter from the
//! `buf.gen.yaml`, which is where the language, the reason registry's path and
//! the package map come from:
//!
//! ```yaml
//! plugins:
//!   - local: path/to/protoc-gen-loams-facade
//!     out: sdks/typescript/packages/client/src/gen
//!     opt:
//!       - lang=typescript
//!       - reasons=docs/api/reasons.md
//!       - package=loams.instance.v1=@loams/proto/instance
//! ```
//!
//! `scripts/sdk/gen.sh <lang>` is the entry point a developer runs; CI runs
//! the same thing and fails on a diff.

use std::io::{Read, Write};
use std::path::Path;

use loams_facade_gen::{Options, go, model_from_request, python, reasons, rust, typescript, wire};

/// Where the generated file lands inside the template's `out` directory, per
/// language. Each renderer writes one aggregate file over the whole proto
/// module, so a language's name is all that differs.
fn file_name(lang: &str) -> Result<&'static str, String> {
    match lang {
        "typescript" => Ok("facade.ts"),
        "python" => Ok("facade.py"),
        "go" => Ok("facade.go"),
        "rust" => Ok("facade.rs"),
        other => Err(format!("no template for {other:?} (SDK1 Task 3)")),
    }
}

/// `CodeGeneratorResponse.supported_features`: `FEATURE_PROTO3_OPTIONAL`, so
/// buf does not warn about optional fields in the descriptors it passes on.
const FEATURE_PROTO3_OPTIONAL: u64 = 1;

fn main() {
    match run() {
        Ok(response) => {
            std::io::stdout()
                .write_all(&response)
                .expect("writing the CodeGeneratorResponse");
        }
        Err(message) => {
            // A protoc plugin reports failure in the response's `error` field,
            // which is what buf prints; the exit status only fails CI earlier.
            let mut response = Vec::new();
            put_bytes(&mut response, 1, message.as_bytes());
            std::io::stdout()
                .write_all(&response)
                .expect("writing the error response");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<Vec<u8>, String> {
    let mut request = Vec::new();
    std::io::stdin()
        .read_to_end(&mut request)
        .map_err(|err| format!("reading the CodeGeneratorRequest: {err}"))?;
    let options = Options::parse(&parameter(&request)?)?;
    let name = file_name(&options.lang)?;
    let reasons = match &options.reasons {
        Some(path) => reasons::read(Path::new(path))?,
        None => Vec::new(),
    };
    let model = model_from_request(&request, reasons).map_err(|err| format!("{err:#}"))?;
    let rendered = match options.lang.as_str() {
        "typescript" => typescript::render(&model, &options.packages, &options.proto_rev)?,
        "python" => python::render(&model, &options.packages, &options.proto_rev)?,
        "go" => go::render(&model, &options.packages, &options.proto_rev)?,
        "rust" => rust::render(&model, &options.packages, &options.proto_rev)?,
        // `file_name` has already rejected anything else.
        other => return Err(format!("no template for {other:?} (SDK1 Task 3)")),
    };

    let mut file = Vec::new();
    put_bytes(&mut file, 1, name.as_bytes());
    put_bytes(&mut file, 15, rendered.as_bytes());
    let mut response = Vec::new();
    put_varint_field(&mut response, 2, FEATURE_PROTO3_OPTIONAL);
    put_bytes(&mut response, 15, &file);
    Ok(response)
}

/// `CodeGeneratorRequest.parameter`, field 2.
fn parameter(request: &[u8]) -> Result<String, String> {
    let mut reader = wire::Reader::new(request);
    while let Some((number, value)) = reader.next_field().map_err(|err| format!("{err:#}"))? {
        if number == 2
            && let wire::Value::Bytes(bytes) = value
        {
            return std::str::from_utf8(bytes)
                .map(str::to_owned)
                .map_err(|_| "the plugin parameter is not UTF-8".to_owned());
        }
    }
    Ok(String::new())
}

/// A length-delimited field.
fn put_bytes(out: &mut Vec<u8>, number: u32, bytes: &[u8]) {
    put_varint(out, (u64::from(number) << 3) | 2);
    put_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

/// A varint field.
fn put_varint_field(out: &mut Vec<u8>, number: u32, value: u64) {
    put_varint(out, u64::from(number) << 3);
    put_varint(out, value);
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_is_a_varint() {
        let mut out = Vec::new();
        put_bytes(&mut out, 1, b"ab");
        assert_eq!(out, vec![0x0a, 2, b'a', b'b']);
    }

    #[test]
    fn the_parameter_is_field_two() {
        // field 2, wire type 2, "lang=typescript".
        let mut request = Vec::new();
        put_bytes(&mut request, 2, b"lang=typescript");
        assert_eq!(parameter(&request).expect("parameter"), "lang=typescript");
        assert_eq!(parameter(&[]).expect("empty"), "");
    }

    /// One generated file per language, named for the language, and an unknown
    /// one fails rather than writing a file nobody asked for.
    #[test]
    fn the_output_file_is_named_for_the_language() {
        assert_eq!(file_name("typescript").expect("ts"), "facade.ts");
        assert_eq!(file_name("python").expect("py"), "facade.py");
        assert_eq!(file_name("go").expect("go"), "facade.go");
        assert_eq!(file_name("rust").expect("rs"), "facade.rs");
        let err = file_name("cobol").expect_err("an unimplemented language must fail");
        assert!(err.contains("no template for \"cobol\""), "{err}");
    }
}
