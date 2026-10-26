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
        json!({"inputRows":4,"acceptedRecords":2,
        "identicalDuplicateRows":0,"quarantinedRows":2})
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
    assert_eq!(payload["quarantine"][0]["reason"], "missing-coordinates");
    assert_eq!(
        payload["quarantine"][1]["reason"],
        "location-error-requires-review"
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
    for (field, value) in [("LatitudeDD", json!(null)), ("LongitudeDD", json!(""))] {
        assert_eq!(reason(field, value), "missing-coordinates");
    }
    for (field, value) in [
        ("LatitudeDD", json!(91)),
        ("LongitudeDD", json!(181)),
        ("LatitudeDD", json!(-97.75)),
        ("LongitudeDD", json!(0)),
        ("LatitudeDD", json!(40)),
    ] {
        assert_eq!(reason(field, value), "coordinates-outside-texas-guardrail");
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
    for value in [json!(1), json!(-1), json!(32767)] {
        assert_eq!(
            reason("LocationError", value),
            "location-error-requires-review"
        );
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
    assert_eq!(records.len(), 2);
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
        "identicalDuplicateRows":1,"quarantinedRows":0})
    );
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
    invalid["attributes"]["LatitudeDD"] = Value::Null;
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
        "missing-coordinates"
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
