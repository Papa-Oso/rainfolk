use gev_us_groundwater::texas_sdr::{MAX_BYTES, MAX_ROWS, normalize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn capture() -> Value {
    serde_json::from_str(include_str!(
        "../../../fixtures/texas-sdr/synthetic-v1.json"
    ))
    .unwrap()
}
fn run(input: &Value) -> Value {
    normalize(&serde_json::to_vec(input).unwrap()).unwrap()
}
fn one() -> Value {
    let mut input = capture();
    input["response"]["features"]
        .as_array_mut()
        .unwrap()
        .truncate(1);
    input
}
fn reason(field: &str, value: Value) -> String {
    let mut input = one();
    input["response"]["features"][0]["attributes"][field] = value;
    let result = run(&input);
    assert_eq!(result["payload"]["records"], json!([]));
    result["payload"]["quarantine"][0]["reason"]
        .as_str()
        .unwrap()
        .into()
}
fn digest(value: &Value) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(value).unwrap()))
}

#[test]
fn synthetic_fixture_normalizes_and_accounts_for_every_row() {
    let result = run(&capture());
    let payload = &result["payload"];
    assert_eq!(
        payload["accounting"],
        json!({"inputRows":4,"acceptedRecords":4,
        "identicalDuplicateRows":0,"quarantinedRows":0,"mapEligibleRecords":3,"nonmappedRecords":1})
    );
    let record = &payload["records"][0];
    assert_eq!(record["id"], "TWDB-SDR-000000000000001");
    assert_eq!(record["sourceRecordId"], "000000000000001");
    assert_eq!(record["proposedUse"], "Domestic");
    assert_eq!(record["boreholeDepth"], json!({"value":125.5,"unit":"ft"}));
    assert_eq!(
        record["completedAt"],
        json!({"unixMilliseconds":1709164800000_i64,
        "timeZone":"UTC","precision":"source-timestamp"})
    );
    assert_eq!(record["coordinateAccuracy"], "unknown");
    assert_eq!(record["activeWellStatus"], "unverified");
    assert_eq!(record["ownershipStatus"], "not-collected");
    assert_eq!(record["waterSafetyStatus"], "not-assessed");
    assert_eq!(payload["coverage"]["complete"], false);
    assert_eq!(payload["coverage"]["exceededTransferLimit"], true);
    assert_eq!(result["manifest"]["activation"], "offline-review-only");
    assert_eq!(
        payload["records"][2]["coordinateQuality"],
        "missing-coordinates"
    );
    assert_eq!(payload["records"][2]["mapEligible"], false);
    assert_eq!(payload["records"][3]["locationError"], 1);
    assert_eq!(payload["records"][3]["mapEligible"], true);
    assert!(
        payload["records"][3]["locationQualityWarning"]
            .as_str()
            .unwrap()
            .contains("uninterpreted")
    );
}

#[test]
fn missing_optional_values_stay_null_and_zero_depth_stays_zero() {
    let result = run(&capture());
    let record = &result["payload"]["records"][1];
    assert!(record["proposedUse"].is_null());
    assert!(record["boreholeDepth"]["value"].is_null());
    assert!(record["completedAt"].is_null());
    assert!(record["locationError"].is_null());
    let mut input = one();
    let attributes = &mut input["response"]["features"][0]["attributes"];
    attributes["BoreholeDepthFt"] = json!(0);
    attributes["ProposedUse"] = json!(" ");
    attributes["DateOfWellCompletion"] = json!("");
    let result = run(&input);
    assert_eq!(
        result["payload"]["records"][0]["boreholeDepth"]["value"],
        0.0
    );
    assert!(result["payload"]["records"][0]["proposedUse"].is_null());
    assert!(result["payload"]["records"][0]["completedAt"].is_null());
}

#[test]
fn coordinates_are_never_swapped_coerced_or_invented() {
    for (field, value, quality) in [
        ("LatitudeDD", json!(null), "missing-coordinates"),
        ("LongitudeDD", json!(""), "missing-coordinates"),
        (
            "LatitudeDD",
            json!(91),
            "coordinates-outside-texas-guardrail",
        ),
        (
            "LongitudeDD",
            json!(181),
            "coordinates-outside-texas-guardrail",
        ),
        (
            "LatitudeDD",
            json!(-97.75),
            "coordinates-outside-texas-guardrail",
        ),
        (
            "LongitudeDD",
            json!(0),
            "coordinates-outside-texas-guardrail",
        ),
        (
            "LatitudeDD",
            json!(40),
            "coordinates-outside-texas-guardrail",
        ),
    ] {
        let mut input = one();
        input["response"]["features"][0]["attributes"][field] = value.clone();
        let result = run(&input);
        let record = &result["payload"]["records"][0];
        assert_eq!(record["mapEligible"], false);
        assert!(record["latitude"].is_null());
        assert!(record["longitude"].is_null());
        assert_eq!(record["coordinateQuality"], quality);
        assert_eq!(
            record["sourceCoordinates"][if field == "LatitudeDD" {
                "latitude"
            } else {
                "longitude"
            }],
            value
        );
        assert_eq!(result["payload"]["accounting"]["acceptedRecords"], 1);
        assert_eq!(result["payload"]["accounting"]["nonmappedRecords"], 1);
    }
    for value in [
        json!("NaN"),
        json!("30.25"),
        json!(true),
        json!({}),
        json!([]),
    ] {
        assert_eq!(reason("LatitudeDD", value), "invalid-number");
    }
}

#[test]
fn location_error_is_preserved_without_an_accuracy_claim() {
    for value in [json!(null), json!(0), json!(1), json!(-1), json!(32767)] {
        let mut input = one();
        input["response"]["features"][0]["attributes"]["LocationError"] = value.clone();
        let result = run(&input);
        let record = &result["payload"]["records"][0];
        assert_eq!(record["locationError"], value);
        assert_eq!(record["mapEligible"], true);
        assert_eq!(record["coordinateAccuracy"], "unknown");
        assert_eq!(record["coordinateVerification"], "unverified");
        assert_eq!(result["payload"]["accounting"]["acceptedRecords"], 1);
    }
    for value in [json!(1.5), json!(true), json!("0"), json!(32768)] {
        assert_eq!(reason("LocationError", value), "invalid-location-error");
    }
}

#[test]
fn malformed_dates_depths_uses_and_ids_are_quarantined() {
    for value in [
        json!("2024-02-30"),
        json!(1709164800000.5),
        json!(true),
        json!(253402300800000_i64),
    ] {
        assert_eq!(
            reason("DateOfWellCompletion", value),
            "invalid-completion-date"
        );
    }
    assert_eq!(
        reason("BoreholeDepthFt", json!(-1)),
        "negative-borehole-depth"
    );
    assert_eq!(reason("BoreholeDepthFt", json!("125.5")), "invalid-number");
    assert_eq!(
        reason("ProposedUse", json!("x".repeat(51))),
        "invalid-proposed-use"
    );
    assert_eq!(
        reason("ProposedUse", json!("Domestic\n")),
        "invalid-proposed-use"
    );
    for value in [
        json!(null),
        json!(1),
        json!(""),
        json!(" 0001"),
        json!("USGS-0001"),
        json!("1".repeat(16)),
    ] {
        assert_eq!(
            reason("WellReportTrackingNumber", value),
            "invalid-report-id"
        );
    }
}

#[test]
fn same_location_distinct_source_ids_remain_distinct() {
    let result = run(&capture());
    let records = result["payload"]["records"].as_array().unwrap();
    assert_eq!(records.len(), 4);
    assert_ne!(records[0]["id"], records[1]["id"]);
    assert_eq!(records[0]["latitude"], records[1]["latitude"]);
    assert_eq!(records[0]["longitude"], records[1]["longitude"]);
    // This construction candidate cannot silently enter the USGS bundle/index contract.
    assert!(gev_us_groundwater::validate_bundle(&result).is_err());
}

#[test]
fn identical_reports_deduplicate_only_by_source_id() {
    let mut input = one();
    let row = input["response"]["features"][0].clone();
    input["response"]["features"]
        .as_array_mut()
        .unwrap()
        .push(row);
    let result = run(&input);
    assert_eq!(
        result["payload"]["accounting"],
        json!({"inputRows":2,"acceptedRecords":1,
        "identicalDuplicateRows":1,"quarantinedRows":0,"mapEligibleRecords":1,"nonmappedRecords":0})
    );
}

#[test]
fn signed_zero_depth_is_canonical_before_duplicate_comparison_and_hashing() {
    let mut input = one();
    let template = input["response"]["features"][0].clone();
    let rows = [json!(-0.0), json!(0.0), json!(0)].map(|depth| {
        let mut row = template.clone();
        row["attributes"]["BoreholeDepthFt"] = depth;
        row
    });
    input["response"]["features"] = json!(rows);
    let first = run(&input);
    input["response"]["features"]
        .as_array_mut()
        .unwrap()
        .reverse();
    let reversed = run(&input);
    let first_record = &first["payload"]["records"][0];
    let reversed_record = &reversed["payload"]["records"][0];
    assert_eq!(
        first["payload"]["accounting"],
        json!({"inputRows":3,"acceptedRecords":1,
        "identicalDuplicateRows":2,"quarantinedRows":0,"mapEligibleRecords":1,"nonmappedRecords":0})
    );
    assert_eq!(
        serde_json::to_string(&first_record["boreholeDepth"]).unwrap(),
        "{\"unit\":\"ft\",\"value\":0.0}"
    );
    assert_eq!(
        first_record["boreholeDepth"],
        reversed_record["boreholeDepth"]
    );
    assert_eq!(
        first_record["contentSha256"],
        reversed_record["contentSha256"]
    );
    // Source byte/provenance hashes should still distinguish reordered captures.
    assert_ne!(
        first["manifest"]["input"]["sha256"],
        reversed["manifest"]["input"]["sha256"]
    );
}

#[test]
fn duplicate_json_members_are_rejected_before_projection_or_normalization() {
    let input = serde_json::to_string(&one()).unwrap();
    let response = serde_json::to_string(&one()["response"]).unwrap();
    let attributes =
        serde_json::to_string(&one()["response"]["features"][0]["attributes"]).unwrap();
    let features = serde_json::to_string(&one()["response"]["features"]).unwrap();
    let cases = [
        input.replace("\"BoreholeDepthFt\":125.5", "\"BoreholeDepthFt\":100,\"BoreholeDepthFt\":125.5"),
        input.replace("\"BoreholeDepthFt\":125.5", "\"BoreholeDepthFt\":125.5,\"BoreholeDepthFt\":125.5"),
        input.replace("\"synthetic\":true", "\"synthetic\":false,\"synthetic\":true"),
        input.replace("\"synthetic\":true", "\"synthetic\":true,\"synthetic\":false"),
        input.replace("\"synthetic\":true", r#""synthetic":false,"\u0073ynthetic":true"#),
        input.replace(&format!("\"response\":{response}"),
            &format!("\"response\":{{\"private\":\"PRIVATE-SENTINEL\"}},\"response\":{response}")),
        input.replace(&format!("\"attributes\":{attributes}"),
            &format!("\"attributes\":{{\"WellOwner\":\"PRIVATE-SENTINEL\"}},\"attributes\":{attributes}")),
        input.replace(&format!("\"features\":{features}"),
            &format!("\"features\":[{{\"attributes\":{{\"WellOwner\":\"PRIVATE-SENTINEL\"}}}}],\"features\":{features}")),
        input.replace("\"wkid\":4326", "\"wkid\":3857,\"wkid\":4326"),
        // Also inspect unknown nested objects, rather than skipping their values.
        input.replacen('{', "{\"unknown\":[{\"hidden\":1,\"hidden\":2}],", 1),
    ];
    for invalid in cases {
        assert_ne!(
            invalid, input,
            "regression setup did not insert duplicate members"
        );
        let error = normalize(invalid.as_bytes()).unwrap_err();
        assert_eq!(error, "invalid SDR JSON or duplicate member");
        assert!(!error.contains("PRIVATE-SENTINEL"));
    }
    // JSON strings are values, not JSON member syntax.
    let mut valid = one();
    valid["response"]["features"][0]["attributes"]["ProposedUse"] = json!(r#"{"x":1,"x":2}"#);
    assert!(normalize(&serde_json::to_vec(&valid).unwrap()).is_ok());
}

#[test]
fn conflicting_report_versions_have_no_arrival_order_winner() {
    let mut input = one();
    let mut changed = input["response"]["features"][0].clone();
    changed["attributes"]["BoreholeDepthFt"] = json!(200);
    input["response"]["features"]
        .as_array_mut()
        .unwrap()
        .push(changed);
    for _ in 0..2 {
        let result = run(&input);
        assert_eq!(result["payload"]["accounting"]["acceptedRecords"], 0);
        assert_eq!(result["payload"]["accounting"]["quarantinedRows"], 2);
        assert!(
            result["payload"]["quarantine"]
                .as_array()
                .unwrap()
                .iter()
                .all(|q| q["reason"] == "conflicting-report-id")
        );
        input["response"]["features"]
            .as_array_mut()
            .unwrap()
            .reverse();
    }
}

#[test]
fn invalid_version_prevents_admission_of_another_row_with_same_id() {
    let mut input = one();
    let mut invalid = input["response"]["features"][0].clone();
    invalid["attributes"]["BoreholeDepthFt"] = json!(-1);
    input["response"]["features"]
        .as_array_mut()
        .unwrap()
        .push(invalid);
    let result = run(&input);
    assert_eq!(result["payload"]["accounting"]["acceptedRecords"], 0);
    assert_eq!(result["payload"]["accounting"]["quarantinedRows"], 2);
    assert_eq!(
        result["payload"]["quarantine"][0]["reason"],
        "conflicting-report-id"
    );
    assert_eq!(
        result["payload"]["quarantine"][1]["reason"],
        "negative-borehole-depth"
    );
}

#[test]
fn hashes_replay_and_separate_source_content_from_capture_provenance() {
    let input = capture();
    let result = run(&input);
    assert_eq!(result, run(&input));
    assert_eq!(result["manifestSha256"], digest(&result["manifest"]));
    assert_eq!(
        result["manifest"]["payload"]["sha256"],
        digest(&result["payload"])
    );
    assert_eq!(
        result["manifest"]["payload"]["bytes"],
        serde_json::to_vec(&result["payload"]).unwrap().len()
    );
    let mut later = input.clone();
    later["retrievedAt"] = json!("2026-10-26T01:00:00Z");
    let later = run(&later);
    assert_eq!(
        result["payload"]["records"][0]["contentSha256"],
        later["payload"]["records"][0]["contentSha256"]
    );
    assert_ne!(
        result["manifest"]["input"]["sha256"],
        later["manifest"]["input"]["sha256"]
    );
    let mut reversed = input;
    reversed["response"]["features"]
        .as_array_mut()
        .unwrap()
        .reverse();
    assert_eq!(
        result["payload"]["records"][0]["contentSha256"],
        run(&reversed)["payload"]["records"][0]["contentSha256"]
    );
}

#[test]
fn projection_rejects_private_and_unknown_fields_without_echoing_values() {
    for field in [
        "WellOwner",
        "WellStreet",
        "WellCity",
        "WellZipCode",
        "Email",
        "ObjectId",
    ] {
        let mut input = capture();
        input["response"]["features"][3]["attributes"][field] = json!("PRIVATE-SENTINEL");
        let error = normalize(&serde_json::to_vec(&input).unwrap()).unwrap_err();
        assert!(!error.contains("PRIVATE-SENTINEL"));
        assert!(!error.contains(field));
    }
    let mut input = one();
    input["response"]["features"][0]["geometry"] = json!({"x":-97.75,"y":30.25});
    assert!(normalize(&serde_json::to_vec(&input).unwrap()).is_err());
}

#[test]
fn envelopes_rights_and_budgets_fail_closed() {
    for (field, value) in [
        ("synthetic", json!(false)),
        ("schemaVersion", json!(2)),
        ("retrievedAt", json!("2026-10-26")),
        ("sourceUrl", json!("https://example.com/")),
    ] {
        let mut input = one();
        input[field] = value;
        assert!(normalize(&serde_json::to_vec(&input).unwrap()).is_err());
    }
    for value in [json!(null), json!(false), json!({"wkid":3857})] {
        let mut input = one();
        input["response"]["spatialReference"] = value;
        assert!(normalize(&serde_json::to_vec(&input).unwrap()).is_err());
    }
    let mut input = one();
    let row = input["response"]["features"][0].clone();
    input["response"]["features"] = json!(vec![row.clone(); MAX_ROWS]);
    assert!(normalize(&serde_json::to_vec(&input).unwrap()).is_ok());
    input["response"]["features"]
        .as_array_mut()
        .unwrap()
        .push(row);
    assert!(normalize(&serde_json::to_vec(&input).unwrap()).is_err());
    assert!(normalize(&vec![b' '; MAX_BYTES + 1]).is_err());
    assert!(normalize(b"{").is_err());
}

#[test]
fn public_capture_is_explicit_bounded_and_distinct_from_synthetic_evidence() {
    let mut input = capture();
    input["kind"] = json!("rainfolk-texas-sdr-public-capture");
    input["synthetic"] = json!(false);
    input["queryUrl"] = json!(gev_us_groundwater::texas_sdr::public_query_url());
    input["sourceResponseSha256"] = json!("a".repeat(64));
    input["sourceSchemaSha256"] = json!("b".repeat(64));
    input["response"]["spatialReferenceBasis"] =
        json!("validated-layer-schema; attributes-unverified");
    let result = run(&input);
    assert_eq!(result["payload"]["synthetic"], false);
    assert_eq!(result["manifest"]["activation"], "local-review-only");
    assert_eq!(result["payload"]["accounting"]["acceptedRecords"], 4);
    assert_eq!(
        result["payload"]["records"][0]["provenance"]["sourceResponseSha256"],
        "a".repeat(64)
    );
    assert_eq!(
        result["payload"]["records"][0]["provenance"]["synthetic"],
        false
    );
    assert_eq!(
        result["payload"]["records"][0]["provenance"]["serviceSpatialReferenceBasis"],
        "validated-layer-schema; attributes-unverified"
    );
    for basis in [Value::Null, json!("inferred-from-coordinates")] {
        let mut bad = input.clone();
        bad["response"]["spatialReferenceBasis"] = basis;
        assert!(normalize(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    for (key, value) in [
        ("queryUrl", json!("https://example.com/")),
        ("sourceSchemaSha256", json!("bad")),
        ("sourceResponseSha256", Value::Null),
        ("synthetic", json!(true)),
    ] {
        let mut bad = input.clone();
        bad[key] = value;
        assert!(normalize(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
}

#[test]
fn retained_raw_coordinate_versions_cannot_have_an_arrival_order_winner() {
    let mut input = one();
    let template = input["response"]["features"][0].clone();
    let rows = [json!(-0.0), json!(0.0)].map(|value| {
        let mut row = template.clone();
        row["attributes"]["LatitudeDD"] = value;
        row
    });
    input["response"]["features"] = json!(rows);
    for _ in 0..2 {
        let result = run(&input);
        assert_eq!(result["payload"]["accounting"]["acceptedRecords"], 0);
        assert_eq!(result["payload"]["accounting"]["quarantinedRows"], 2);
        input["response"]["features"]
            .as_array_mut()
            .unwrap()
            .reverse();
    }
}

#[test]
fn one_row_public_projection_keeps_unrequested_fields_unknown_and_scoped() {
    let mut input = one();
    input["kind"] = json!("rainfolk-texas-sdr-public-capture");
    input["synthetic"] = json!(false);
    let prefix = format!(
        "{}/query?f=pjson&where=1%3D1&outFields=",
        gev_us_groundwater::texas_sdr::SOURCE
    );
    let suffix = "&returnGeometry=false&resultRecordCount=1";
    let query = format!(
        "{prefix}WellReportTrackingNumber%2CProposedUse%2CLatitudeDD%2CLongitudeDD{suffix}"
    );
    input["queryUrl"] = json!(query);
    input["sourceResponseSha256"] = json!("a".repeat(64));
    input["sourceSchemaSha256"] = json!("b".repeat(64));
    input["response"]["spatialReferenceBasis"] = json!("source-response; attributes-unverified");
    for field in ["DateOfWellCompletion", "BoreholeDepthFt", "LocationError"] {
        input["response"]["features"][0]["attributes"]
            .as_object_mut()
            .unwrap()
            .remove(field);
    }
    let output = run(&input);
    let record = &output["payload"]["records"][0];
    assert_eq!(output["payload"]["accounting"]["acceptedRecords"], 1);
    assert!(record["completedAt"].is_null());
    assert!(record["boreholeDepth"]["value"].is_null());
    assert!(record["locationError"].is_null());
    assert_eq!(
        record["provenance"]["unrequestedFields"],
        json!(["DateOfWellCompletion", "BoreholeDepthFt", "LocationError"])
    );
    let mut explained = input.clone();
    explained["fieldLimitations"] = json!({"DateOfWellCompletion":"source-query-api-400"});
    explained["fieldLimitationsEvidenceSha256"] = json!("c".repeat(64));
    assert_eq!(
        run(&explained)["payload"]["records"][0]["provenance"]["fieldLimitations"]["DateOfWellCompletion"],
        "source-query-api-400"
    );
    for bad in [
        json!({"LatitudeDD":"source-query-api-400"}),
        json!({"DateOfWellCompletion":"unverified-guess"}),
    ] {
        explained["fieldLimitations"] = bad;
        assert!(normalize(&serde_json::to_vec(&explained).unwrap()).is_err());
    }
    for bad_url in [
        query.replace("resultRecordCount=1", "resultRecordCount=2"),
        query.replace("pjson", "json"),
        format!("{prefix}WellReportTrackingNumber%2CWellOwner{suffix}"),
        format!("{prefix}LatitudeDD{suffix}"),
        format!("{prefix}WellReportTrackingNumber%2CLatitudeDD%2CLatitudeDD{suffix}"),
    ] {
        let mut bad = input.clone();
        bad["queryUrl"] = json!(bad_url);
        assert!(normalize(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    let mut too_many = input.clone();
    let row = too_many["response"]["features"][0].clone();
    too_many["response"]["features"]
        .as_array_mut()
        .unwrap()
        .push(row);
    assert!(normalize(&serde_json::to_vec(&too_many).unwrap()).is_err());
    input["response"]["features"][0]["attributes"]["BoreholeDepthFt"] = json!(125);
    assert!(normalize(&serde_json::to_vec(&input).unwrap()).is_err());
}

fn public_batch(rows: usize) -> Value {
    let mut input = one();
    input["kind"] = json!("rainfolk-texas-sdr-public-capture");
    input["synthetic"] = json!(false);
    input["queryUrl"] = json!(gev_us_groundwater::texas_sdr::public_batch_query_url());
    input["sourceResponseSha256"] = json!("a".repeat(64));
    input["sourceSchemaSha256"] = json!("b".repeat(64));
    input["fieldLimitations"] = json!({"DateOfWellCompletion":"source-query-api-400"});
    input["fieldLimitationsEvidenceSha256"] = json!("c".repeat(64));
    input["response"]["spatialReferenceBasis"] = json!("source-response; attributes-unverified");
    let template = input["response"]["features"][0].clone();
    input["response"]["features"] = (0..rows)
        .map(|i| {
            let mut row = template.clone();
            let attributes = row["attributes"].as_object_mut().unwrap();
            attributes.remove("DateOfWellCompletion");
            attributes.insert("WellReportTrackingNumber".into(), json!(format!("{i:015}")));
            attributes.insert("LocationError".into(), json!(i as i64 - 100));
            if i == 1 {
                attributes.insert("LatitudeDD".into(), Value::Null);
            }
            row
        })
        .collect();
    input
}

#[test]
fn no_date_batch_preserves_unknown_dates_flags_and_deterministic_source_identity() {
    let mut input = public_batch(200);
    let output = run(&input);
    assert_eq!(output, run(&input));
    assert_eq!(
        output["payload"]["accounting"],
        json!({"inputRows":200,"acceptedRecords":200,
        "identicalDuplicateRows":0,"quarantinedRows":0,"mapEligibleRecords":199,"nonmappedRecords":1})
    );
    let records = output["payload"]["records"].as_array().unwrap();
    assert_eq!(records[0]["id"], "TWDB-SDR-000000000000000");
    assert_eq!(records[0]["locationError"], -100);
    assert_eq!(records[0]["mapEligible"], true);
    assert_eq!(records[1]["mapEligible"], false);
    for record in records {
        assert!(record["completedAt"].is_null());
        assert_eq!(record["coordinateAccuracy"], "unknown");
        assert_eq!(
            record["provenance"]["unrequestedFields"],
            json!(["DateOfWellCompletion"])
        );
        assert_eq!(
            record["provenance"]["fieldLimitationsEvidenceSha256"],
            "c".repeat(64)
        );
        assert_eq!(
            record["provenance"]["fieldLimitations"]["DateOfWellCompletion"],
            "source-query-api-400"
        );
    }
    input["response"]["features"]
        .as_array_mut()
        .unwrap()
        .reverse();
    let reversed = run(&input);
    let content = |output: &Value| {
        output["payload"]["records"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (r["id"].clone(), r["contentSha256"].clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(content(&output), content(&reversed));
}

#[test]
fn no_date_batch_refuses_query_drift_extra_rows_dates_and_missing_evidence() {
    let input = public_batch(1);
    let query = gev_us_groundwater::texas_sdr::public_batch_query_url();
    for query in [
        query.replace("resultOffset=0", "resultOffset=200"),
        query.replace("ObjectId%20ASC", "WellReportTrackingNumber"),
        query.replace("resultRecordCount=200", "resultRecordCount=201"),
        query.replace("resultRecordCount=200", "resultRecordCount=199"),
        query.replace("LocationError", "WellOwner"),
    ] {
        let mut bad = input.clone();
        bad["queryUrl"] = json!(query);
        assert!(normalize(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    for field in ["fieldLimitations", "fieldLimitationsEvidenceSha256"] {
        let mut bad = input.clone();
        bad.as_object_mut().unwrap().remove(field);
        assert!(normalize(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    let mut bad = input.clone();
    bad["response"]["features"][0]["attributes"]["DateOfWellCompletion"] = Value::Null;
    assert!(normalize(&serde_json::to_vec(&bad).unwrap()).is_err());
    assert!(normalize(&serde_json::to_vec(&public_batch(201)).unwrap()).is_err());
    assert!(normalize(&vec![b' '; MAX_BYTES + 1]).is_err());
}

#[test]
fn no_date_batch_short_or_empty_page_never_claims_complete_coverage() {
    for rows in [0, 3, 200] {
        for transfer in [false, true] {
            let mut input = public_batch(rows);
            input["response"]["exceededTransferLimit"] = json!(transfer);
            let output = run(&input);
            assert_eq!(output["payload"]["coverage"]["complete"], false);
            assert_eq!(
                output["payload"]["coverage"]["exceededTransferLimit"],
                transfer
            );
            assert_eq!(output["payload"]["accounting"]["inputRows"], rows);
            assert_eq!(output["payload"]["accounting"]["acceptedRecords"], rows);
        }
    }
}

#[test]
fn cli_reads_fixture_without_network_and_rejects_bad_mode() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(env!("CARGO_BIN_EXE_gev-us-groundwater"))
        .arg("--texas-sdr-v1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&capture()).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        run(&capture())
    );
    assert!(
        !Command::new(env!("CARGO_BIN_EXE_gev-us-groundwater"))
            .args(["--texas-sdr-v1", "unexpected"])
            .output()
            .unwrap()
            .status
            .success()
    );
}
