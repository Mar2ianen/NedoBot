use url::{Origin, Url};

#[test]
fn nested_blob_url_has_opaque_origin() {
    let nested = Url::parse("blob:blob:https://example.com/id").expect("valid blob URL");
    assert!(matches!(nested.origin(), Origin::Opaque(_)));

    let deep = format!("{}https://example.com/id", "blob:".repeat(256));
    let nested = Url::parse(&deep).expect("valid nested blob URL");
    assert!(matches!(nested.origin(), Origin::Opaque(_)));
}

#[test]
fn ordinary_blob_url_preserves_https_origin() {
    let blob = Url::parse("blob:https://example.com/id").expect("valid blob URL");
    assert_eq!(
        blob.origin().ascii_serialization(),
        "https://example.com"
    );
}
