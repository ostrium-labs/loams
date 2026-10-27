//! ClickHouse type rules, values and lazy conditionals (HE1 Task 24).
#![allow(clippy::unwrap_used)]
use loams_house_engine::{catalog::MemoryCatalog,execute::execute};
use serde_json::{Value,json};
async fn query(sql:&str)->Vec<Vec<Value>> {
    let output=execute(sql,&[],"default",&MemoryCatalog::default()).await.unwrap();
    String::from_utf8(output).unwrap().lines().map(|l|serde_json::from_str(l).unwrap()).collect()
}
#[tokio::test]
async fn arithmetic_result_types_match_the_oracle(){
    let rows=query("SELECT CAST(255,'UInt8')+CAST(1,'UInt8') AS add, CAST(0,'UInt8')-CAST(1,'UInt8') AS sub, CAST(18446744073709551615,'UInt64')+CAST(1,'UInt64') AS wrapped, CAST(2500,'UInt32')/1000 AS ratio").await;
    assert_eq!(rows[1],json!(["UInt16","Int16","UInt64","Float64"]));
    assert_eq!(rows[2],json!([256,-1,0,2.5]));
}
#[tokio::test]
async fn comparisons_are_exact_across_signed_unsigned_and_float(){
    let rows=query("SELECT CAST(18446744073709551615,'UInt64') > CAST(-1,'Int64') AS signed, CAST(9007199254740993,'UInt64') > CAST(9007199254740992,'Float64') AS rounded, CAST(9007199254740992,'Float64') = CAST(9007199254740993,'UInt64') AS equal").await;
    assert_eq!(rows[1],json!(["UInt8","UInt8","UInt8"]));assert_eq!(rows[2],json!([1,1,0]));
}
#[tokio::test]
async fn boolean_contexts_take_numbers(){
    let rows=query("SELECT -3 AND 5 AS a, 0 OR 2 AS b, NOT -1 AS c WHERE -1").await;
    assert_eq!(rows[1],json!(["UInt8","UInt8","UInt8"]));assert_eq!(rows[2],json!([1,1,0]));
    assert_eq!(query("SELECT 1 WHERE 0").await.len(),2);
}
#[tokio::test]
async fn null_logic_uses_three_valued_truth(){
    let rows=query("SELECT NULL AND 0 AS a,NULL OR 1 AS b,NULL AND 1 AS c,NULL OR 0 AS d,NOT NULL AS e,NULL+1 AS f").await;
    assert_eq!(rows[1],json!(["Nullable(UInt8)","Nullable(UInt8)","Nullable(UInt8)","Nullable(UInt8)","Nullable(Nothing)","Nullable(Nothing)"]));
    assert_eq!(rows[2],json!([0,1,null,null,null,null]));
}
#[tokio::test]
async fn nullable_cast_metadata_survives_constant_folding(){
    let rows=query("SELECT toTypeName(CAST(1,'Nullable(UInt8)')) AS t,CAST(NULL,'Nullable(UInt8)')+CAST(1,'UInt8') AS n,CAST(-1,'UInt8') AS wrapped").await;
    assert_eq!(rows[2],json!(["Nullable(UInt8)",null,255]));assert_eq!(rows[1][1],"Nullable(UInt16)");
}
#[tokio::test]
async fn conditional_branches_use_the_least_supertype(){
    let rows=query("SELECT if(1,CAST(255,'UInt8'),CAST(-1,'Int8')) AS a,CASE WHEN 0 THEN CAST(1,'UInt16') ELSE CAST(-1,'Int8') END AS b,coalesce(CAST(NULL,'Nullable(UInt8)'),CAST(2,'UInt16')) AS c").await;
    assert_eq!(rows[1],json!(["Int16","Int32","UInt16"]));assert_eq!(rows[2],json!([255,-1,2]));
}
#[tokio::test]
async fn conditional_branches_are_lazy(){
    let rows=query("SELECT if(1,7,intDiv(1,0)) AS a,multiIf(0,intDiv(1,0),1,9,intDiv(2,0)) AS b").await;
    assert_eq!(rows[2],json!([7,9]));
}
#[tokio::test]
async fn division_errors_keep_clickhouse_codes(){
    for sql in ["SELECT intDiv(1,0)","SELECT CAST(-9223372036854775808,'Int64') % CAST(-1,'Int8')"] {
        let error=execute(sql,&[],"default",&MemoryCatalog::default()).await.unwrap_err();assert_eq!(error.code,153,"{sql}: {error}");
    }
}
#[tokio::test]
async fn decimal_scale_and_modulo_match_the_oracle(){
    let rows=query("SELECT CAST(7,'Decimal32(2)')+CAST(2,'Decimal64(3)') AS a,CAST(7,'Decimal32(2)')*CAST(2,'Decimal64(3)') AS b,CAST(7,'Decimal128(4)')%CAST(2,'Decimal32(2)') AS c").await;
    assert_eq!(rows[1],json!(["Decimal(18, 3)","Decimal(18, 5)","Decimal(38, 4)"]));assert_eq!(rows[2],json!([9,14,0]));
}
#[tokio::test]
async fn array_elements_share_a_lossless_type(){
    let rows=query("SELECT [CAST(255,'UInt8'),CAST(-1,'Int8')] AS a").await;
    assert_eq!(rows[1],json!(["Array(Int16)"]));assert_eq!(rows[2],json!([[255,-1]]));
}
#[tokio::test]
async fn wide_conditional_types_are_explicit_refusals(){
    let error=execute("SELECT if(1,CAST(1,'UInt64'),CAST(-1,'Int64'))",&[],"default",&MemoryCatalog::default()).await.unwrap_err();assert_eq!(error.code,48);
}
