use anyhow::bail;
use serde_json::{Value, json};
use url::form_urlencoded;

use proxmox_schema::*;

fn parse_query_string<T: Into<ParameterSchema>>(
    query: &str,
    schema: T,
    test_required: bool,
) -> Result<Value, ParameterError> {
    let param_list: Vec<(String, String)> = form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();

    schema
        .into()
        .parse_parameter_strings(&param_list, test_required)
}

#[test]
fn test_schema1() {
    let schema = ObjectSchema::new("TEST", &[]).schema();

    println!("TEST Schema: {schema:?}");
}

#[test]
fn test_query_string() {
    {
        const SCHEMA: ObjectSchema = ObjectSchema::new(
            "Parameters.",
            &[("name", false, &StringSchema::new("Name.").schema())],
        );

        let res = parse_query_string("", &SCHEMA, true);
        assert!(res.is_err());
    }

    {
        const SCHEMA: ObjectSchema = ObjectSchema::new(
            "Parameters.",
            &[("name", true, &StringSchema::new("Name.").schema())],
        );

        let res = parse_query_string("", &SCHEMA, true);
        assert!(res.is_ok());
    }

    // TEST min_length and max_length
    {
        const SCHEMA: ObjectSchema = ObjectSchema::new(
            "Parameters.",
            &[(
                "name",
                true,
                &StringSchema::new("Name.")
                    .min_length(5)
                    .max_length(10)
                    .schema(),
            )],
        );

        let res = parse_query_string("name=abcd", &SCHEMA, true);
        assert!(res.is_err());

        let res = parse_query_string("name=abcde", &SCHEMA, true);
        assert!(res.is_ok());

        let res = parse_query_string("name=abcdefghijk", &SCHEMA, true);
        assert!(res.is_err());

        let res = parse_query_string("name=abcdefghij", &SCHEMA, true);
        assert!(res.is_ok());
    }

    // TEST regex pattern
    crate::const_regex! {
        TEST_REGEX = "test";
        TEST2_REGEX = "^test$";
    }

    {
        const SCHEMA: ObjectSchema = ObjectSchema::new(
            "Parameters.",
            &[(
                "name",
                false,
                &StringSchema::new("Name.")
                    .format(&ApiStringFormat::Pattern(&TEST_REGEX))
                    .schema(),
            )],
        );

        let res = parse_query_string("name=abcd", &SCHEMA, true);
        assert!(res.is_err());

        let res = parse_query_string("name=ateststring", &SCHEMA, true);
        assert!(res.is_ok());
    }

    {
        const SCHEMA: ObjectSchema = ObjectSchema::new(
            "Parameters.",
            &[(
                "name",
                false,
                &StringSchema::new("Name.")
                    .format(&ApiStringFormat::Pattern(&TEST2_REGEX))
                    .schema(),
            )],
        );

        let res = parse_query_string("name=ateststring", &SCHEMA, true);
        assert!(res.is_err());

        let res = parse_query_string("name=test", &SCHEMA, true);
        assert!(res.is_ok());
    }

    // TEST string enums
    {
        const SCHEMA: ObjectSchema = ObjectSchema::new(
            "Parameters.",
            &[(
                "name",
                false,
                &StringSchema::new("Name.")
                    .format(&ApiStringFormat::Enum(&[
                        EnumEntry::new("ev1", "desc ev1"),
                        EnumEntry::new("ev2", "desc ev2"),
                    ]))
                    .schema(),
            )],
        );

        let res = parse_query_string("name=noenum", &SCHEMA, true);
        assert!(res.is_err());

        let res = parse_query_string("name=ev1", &SCHEMA, true);
        assert!(res.is_ok());

        let res = parse_query_string("name=ev2", &SCHEMA, true);
        assert!(res.is_ok());

        let res = parse_query_string("name=ev3", &SCHEMA, true);
        assert!(res.is_err());
    }
}

#[test]
fn test_query_integer() {
    {
        const SCHEMA: ObjectSchema = ObjectSchema::new(
            "Parameters.",
            &[("count", false, &IntegerSchema::new("Count.").schema())],
        );

        let res = parse_query_string("", &SCHEMA, true);
        assert!(res.is_err());
    }

    {
        const SCHEMA: ObjectSchema = ObjectSchema::new(
            "Parameters.",
            &[(
                "count",
                true,
                &IntegerSchema::new("Count.")
                    .minimum(-3)
                    .maximum(50)
                    .schema(),
            )],
        );

        let res = parse_query_string("", &SCHEMA, true);
        assert!(res.is_ok());

        let res = parse_query_string("count=abc", &SCHEMA, false);
        assert!(res.is_err());

        let res = parse_query_string("count=30", &SCHEMA, false);
        assert!(res.is_ok());

        let res = parse_query_string("count=-1", &SCHEMA, false);
        assert!(res.is_ok());

        let res = parse_query_string("count=300", &SCHEMA, false);
        assert!(res.is_err());

        let res = parse_query_string("count=-30", &SCHEMA, false);
        assert!(res.is_err());

        let res = parse_query_string("count=50", &SCHEMA, false);
        assert!(res.is_ok());

        let res = parse_query_string("count=-3", &SCHEMA, false);
        assert!(res.is_ok());
    }
}

#[test]
fn test_query_boolean() {
    {
        const SCHEMA: ObjectSchema = ObjectSchema::new(
            "Parameters.",
            &[("force", false, &BooleanSchema::new("Force.").schema())],
        );

        let res = parse_query_string("", &SCHEMA, true);
        assert!(res.is_err());
    }

    {
        const SCHEMA: ObjectSchema = ObjectSchema::new(
            "Parameters.",
            &[("force", true, &BooleanSchema::new("Force.").schema())],
        );

        let res = parse_query_string("", &SCHEMA, true);
        assert!(res.is_ok());

        let res = parse_query_string("a=b", &SCHEMA, true);
        assert!(res.is_err());

        let res = parse_query_string("force", &SCHEMA, true);
        assert!(res.is_err());

        let res = parse_query_string("force=yes", &SCHEMA, true);
        assert!(res.is_ok());
        let res = parse_query_string("force=1", &SCHEMA, true);
        assert!(res.is_ok());
        let res = parse_query_string("force=On", &SCHEMA, true);
        assert!(res.is_ok());
        let res = parse_query_string("force=TRUE", &SCHEMA, true);
        assert!(res.is_ok());
        let res = parse_query_string("force=TREU", &SCHEMA, true);
        assert!(res.is_err());

        let res = parse_query_string("force=NO", &SCHEMA, true);
        assert!(res.is_ok());
        let res = parse_query_string("force=0", &SCHEMA, true);
        assert!(res.is_ok());
        let res = parse_query_string("force=off", &SCHEMA, true);
        assert!(res.is_ok());
        let res = parse_query_string("force=False", &SCHEMA, true);
        assert!(res.is_ok());
    }
}

#[test]
fn test_verify_function() {
    const SCHEMA: ObjectSchema = ObjectSchema::new(
        "Parameters.",
        &[(
            "p1",
            false,
            &StringSchema::new("P1")
                .format(&ApiStringFormat::VerifyFn(|value| {
                    if value == "test" {
                        return Ok(());
                    };
                    bail!("format error");
                }))
                .schema(),
        )],
    );

    let res = parse_query_string("p1=tes", &SCHEMA, true);
    assert!(res.is_err());
    let res = parse_query_string("p1=test", &SCHEMA, true);
    assert!(res.is_ok());
}

#[test]
fn test_verify_complex_object() {
    const NIC_MODELS: ApiStringFormat = ApiStringFormat::Enum(&[
        EnumEntry::new("e1000", "Intel E1000"),
        EnumEntry::new("virtio", "Paravirtualized ethernet device"),
    ]);

    const PARAM_SCHEMA: Schema = ObjectSchema::new(
        "Properties.",
        &[
            (
                "enable",
                true,
                &BooleanSchema::new("Enable device.").schema(),
            ),
            (
                "model",
                false,
                &StringSchema::new("Ethernet device Model.")
                    .format(&NIC_MODELS)
                    .schema(),
            ),
        ],
    )
    .default_key("model")
    .schema();

    const SCHEMA: ObjectSchema = ObjectSchema::new(
        "Parameters.",
        &[(
            "net0",
            false,
            &StringSchema::new("First Network device.")
                .format(&ApiStringFormat::PropertyString(&PARAM_SCHEMA))
                .schema(),
        )],
    );

    let res = parse_query_string("", &SCHEMA, true);
    assert!(res.is_err());

    let res = parse_query_string("test=abc", &SCHEMA, true);
    assert!(res.is_err());

    let res = parse_query_string("net0=model=abc", &SCHEMA, true);
    assert!(res.is_err());

    let res = parse_query_string("net0=model=virtio", &SCHEMA, true);
    assert!(res.is_ok());

    let res = parse_query_string("net0=model=virtio,enable=1", &SCHEMA, true);
    assert!(res.is_ok());

    let res = parse_query_string("net0=virtio,enable=no", &SCHEMA, true);
    assert!(res.is_ok());
}

#[test]
fn test_verify_complex_array() {
    {
        const PARAM_SCHEMA: Schema =
            ArraySchema::new("Integer List.", &IntegerSchema::new("Something").schema()).schema();

        const SCHEMA: ObjectSchema = ObjectSchema::new(
            "Parameters.",
            &[(
                "list",
                false,
                &StringSchema::new("A list on integers, comma separated.")
                    .format(&ApiStringFormat::PropertyString(&PARAM_SCHEMA))
                    .schema(),
            )],
        );

        let res = parse_query_string("", &SCHEMA, true);
        assert!(res.is_err());

        let res = parse_query_string("list=", &SCHEMA, true);
        assert!(res.is_ok());

        let res = parse_query_string("list=abc", &SCHEMA, true);
        assert!(res.is_err());

        let res = parse_query_string("list=1", &SCHEMA, true);
        assert!(res.is_ok());

        let res = parse_query_string("list=2,3,4,5", &SCHEMA, true);
        assert!(res.is_ok());
    }

    {
        const PARAM_SCHEMA: Schema =
            ArraySchema::new("Integer List.", &IntegerSchema::new("Something").schema())
                .min_length(1)
                .max_length(3)
                .schema();

        const SCHEMA: ObjectSchema = ObjectSchema::new(
            "Parameters.",
            &[(
                "list",
                false,
                &StringSchema::new("A list on integers, comma separated.")
                    .format(&ApiStringFormat::PropertyString(&PARAM_SCHEMA))
                    .schema(),
            )],
        );

        let res = parse_query_string("list=", &SCHEMA, true);
        assert!(res.is_err());

        let res = parse_query_string("list=1,2,3", &SCHEMA, true);
        assert!(res.is_ok());

        let res = parse_query_string("list=2,3,4,5", &SCHEMA, true);
        assert!(res.is_err());
    }
}

#[test]
fn test_one_of_schema_string_variant() {
    const OBJECT1_SCHEMA: Schema = ObjectSchema::new(
        "Object 1",
        &[
            ("a", false, &StringSchema::new("A property").schema()),
            ("type", false, &StringSchema::new("v1 or v2").schema()),
        ],
    )
    .schema();
    const OBJECT2_SCHEMA: Schema = ObjectSchema::new(
        "Object 2",
        &[
            (
                "b",
                true,
                &StringSchema::new("A optional property").schema(),
            ),
            ("type", false, &StringSchema::new("v1 or v2").schema()),
        ],
    )
    .schema();

    const NO_STRING_VARIANT_SCHEMA: OneOfSchema = OneOfSchema::new(
        "An oneOf schema",
        &("type", false, &StringSchema::new("v1 or v2").schema()),
        &[("v1", &OBJECT1_SCHEMA), ("v2", &OBJECT2_SCHEMA)],
    );

    const ONE_STRING_VARIANT_SCHEMA: OneOfSchema = OneOfSchema::new(
        "An oneOf schema with a string variant",
        &(
            "type",
            false,
            &StringSchema::new("string or v1 or v2").schema(),
        ),
        &[
            (
                "name does not matter",
                &StringSchema::new("A string").schema(),
            ),
            ("v1", &OBJECT1_SCHEMA),
            ("v2", &OBJECT2_SCHEMA),
        ],
    );

    NO_STRING_VARIANT_SCHEMA
        .verify_json(&json!({
            "type": "v1", "a": "foo"
        }))
        .expect("should verify");

    ONE_STRING_VARIANT_SCHEMA
        .verify_json(&json!({
            "type": "v2", "b": "foo"
        }))
        .expect("should verify");

    ONE_STRING_VARIANT_SCHEMA
        .verify_json(&json!("plain string"))
        .expect("should verify");
}

#[test]
fn test_property_aliases() {
    const SCHEMA: ObjectSchema = ObjectSchema::new(
        "Parameters.",
        &[
            ("count", true, &IntegerSchema::new("Count.").schema()),
            ("mode", false, &StringSchema::new("Mode.").schema()),
            (
                "tags",
                true,
                &ArraySchema::new("Tags.", &StringSchema::new("Tag.").schema()).schema(),
            ),
        ],
    )
    .property_aliases(&[
        ("deprecated-mode", "mode"),
        ("legacy-mode", "mode"),
        ("nr", "count"),
        ("tag", "tags"),
    ]);

    let res = parse_query_string("mode=fast", &SCHEMA, true).expect("canonical");
    assert_eq!(res["mode"], "fast");

    let res = parse_query_string("deprecated-mode=fast", &SCHEMA, true).expect("alias");
    assert_eq!(res["mode"], "fast");
    assert!(
        res.get("deprecated-mode").is_none(),
        "alias key must be rewritten"
    );

    let res = parse_query_string("deprecated-mode=fast&nr=3", &SCHEMA, true).expect("aliases");
    assert_eq!(res["mode"], "fast");
    assert_eq!(res["count"], 3);

    let err =
        parse_query_string("mode=fast&deprecated-mode=slow", &SCHEMA, true).expect_err("conflict");
    assert!(
        format!("{err}").contains("cannot set both"),
        "unexpected error: {err}"
    );

    // Reverse order: alias first, canonical second. Must report the same conflict, not the
    // generic "duplicate parameter" message.
    let err = parse_query_string("deprecated-mode=fast&mode=slow", &SCHEMA, true)
        .expect_err("reverse-order conflict");
    let err_text = format!("{err}");
    assert!(
        err_text.contains("cannot set both"),
        "reverse-order should report `cannot set both`, got: {err_text}"
    );
    assert!(
        !err_text.contains("duplicate parameter"),
        "reverse-order must not regress to `duplicate parameter`, got: {err_text}"
    );

    // Two distinct aliases of the same canonical: also rejected.
    let err = parse_query_string("deprecated-mode=fast&legacy-mode=slow", &SCHEMA, true)
        .expect_err("two-alias conflict");
    assert!(
        format!("{err}").contains("cannot set both"),
        "two aliases of same canonical must conflict: {err}"
    );

    // Array property via an alias works, and mixing alias with canonical for an array is also
    // rejected (the bug that the array branch previously merged silently).
    let res =
        parse_query_string("mode=fast&tag=foo&tag=bar", &SCHEMA, true).expect("array via alias");
    assert_eq!(res["tags"], json!(["foo", "bar"]));
    let err =
        parse_query_string("mode=fast&tag=foo&tags=bar", &SCHEMA, true).expect_err("array mix");
    assert!(
        format!("{err}").contains("cannot set both"),
        "array branch must reject set-both: {err}"
    );

    SCHEMA
        .verify_json(&json!({"mode": "fast"}))
        .expect("canonical verify");
    SCHEMA
        .verify_json(&json!({"deprecated-mode": "fast"}))
        .expect("alias satisfies required");
    SCHEMA
        .verify_json(&json!({"mode": "fast", "deprecated-mode": "slow"}))
        .expect_err("conflict via verify_json");

    // canonicalize_aliases rewrites the alias key in place, so downstream consumers that look up
    // only the canonical name see the value. This is what the REST server now does before
    // dispatching the macro-generated handler.
    let mut body = json!({"deprecated-mode": "fast"});
    SCHEMA
        .canonicalize_aliases(&mut body)
        .expect("canonicalize rewrites alias");
    assert_eq!(body["mode"], "fast");
    assert!(body.get("deprecated-mode").is_none());

    // canonicalize_aliases catches both-set and leaves the value unchanged.
    let mut body = json!({"deprecated-mode": "fast", "mode": "slow"});
    let original = body.clone();
    SCHEMA
        .canonicalize_aliases(&mut body)
        .expect_err("canonicalize rejects both-set");
    assert_eq!(body, original, "value must not be mutated on conflict");

    // Same-canonical via two different aliases is also caught.
    let mut body = json!({"deprecated-mode": "fast", "legacy-mode": "slow"});
    SCHEMA
        .canonicalize_aliases(&mut body)
        .expect_err("canonicalize rejects two-alias");
}

#[test]
fn test_property_aliases_in_all_of() {
    const LEFT: Schema = ObjectSchema::new(
        "Left.",
        &[("mode", false, &StringSchema::new("Mode.").schema())],
    )
    .property_aliases(&[("legacy-mode", "mode")])
    .schema();
    const RIGHT: Schema = ObjectSchema::new(
        "Right.",
        &[("count", true, &IntegerSchema::new("Count.").schema())],
    )
    .schema();
    const COMBINED: AllOfSchema = AllOfSchema::new("Combined.", &[&LEFT, &RIGHT]);

    let res = parse_query_string("legacy-mode=fast", &COMBINED, true).expect("alias in allof");
    assert_eq!(res["mode"], "fast");

    let mut body = json!({"legacy-mode": "fast", "count": 1});
    COMBINED
        .canonicalize_aliases(&mut body)
        .expect("canonicalize via AllOf");
    assert_eq!(body["mode"], "fast");
    assert!(body.get("legacy-mode").is_none());
}

#[test]
fn test_property_aliases_in_one_of_with_string_variant() {
    // A oneOf may carry a plain-string variant alongside object variants. Canonicalizing
    // aliases must skip that variant instead of unwrapping it as an object and panicking.
    const OBJ: Schema = ObjectSchema::new(
        "Obj.",
        &[
            ("mode", false, &StringSchema::new("Mode.").schema()),
            ("type", false, &StringSchema::new("variant").schema()),
        ],
    )
    .property_aliases(&[("legacy-mode", "mode")])
    .schema();
    const SCHEMA: OneOfSchema = OneOfSchema::new(
        "A string or some object.",
        &("type", false, &StringSchema::new("variant").schema()),
        &[
            ("obj", &OBJ),
            ("plain", &StringSchema::new("A plain string.").schema()),
        ],
    );

    let mut body = json!({"type": "obj", "legacy-mode": "fast"});
    SCHEMA
        .canonicalize_aliases(&mut body)
        .expect("string variant skipped, alias rewritten");
    assert_eq!(body["mode"], "fast");
    assert!(body.get("legacy-mode").is_none());
}

#[test]
#[should_panic(expected = "oneOf can have only zero or one string variants")]
fn test_one_of_schema_with_multiple_string_variant() {
    const OBJECT1_SCHEMA: Schema = ObjectSchema::new(
        "Object 1",
        &[
            ("a", false, &StringSchema::new("A property").schema()),
            ("type", false, &StringSchema::new("v1 or v2").schema()),
        ],
    )
    .schema();
    const TYPE_SCHEMA: Schema = StringSchema::new("string or string or v1").schema();
    const STRING1_SCHEMA: Schema = StringSchema::new("A string").schema();
    const STRING2_SCHEMA: Schema = StringSchema::new("Another string").schema();

    let _ = OneOfSchema::new(
        "An invalid oneOf schema with multiple string variant",
        &("type", false, &TYPE_SCHEMA),
        &[
            ("string variant 1", &STRING1_SCHEMA),
            ("v1", &OBJECT1_SCHEMA),
            ("whoops", &STRING2_SCHEMA),
        ],
    );
}
