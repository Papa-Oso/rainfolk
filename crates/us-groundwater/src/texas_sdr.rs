//! Offline, synthetic-only Texas SDR construction reports. No transport or index activation.
//! Reuses the producer's compact JSON encoding, SHA-256 and artifact descriptors.
use super::{artifact, encoded, sha256, time_precision};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const SOURCE: &str =
    "https://services.twdb.texas.gov/arcgis/rest/services/Public/WellReports/FeatureServer/0";
pub const MAX_ROWS: usize = 200;
pub const MAX_BYTES: usize = 1024 * 1024;
pub const FIELDS: [&str; 7] = [
    "WellReportTrackingNumber",
    "ProposedUse",
    "LatitudeDD",
    "LongitudeDD",
    "DateOfWellCompletion",
    "BoreholeDepthFt",
    "LocationError",
];

fn only_keys(value: &Value, allowed: &[&str]) -> Result<(), String> {
    let object = value.as_object().ok_or("expected object")?;
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        // Never echo unapproved field names or values into logs/quarantine.
        return Err("unapproved field; minimum-field projection required".into());
    }
    Ok(())
}

fn missing(value: &Value) -> bool {
    value.is_null() || value.as_str().is_some_and(|s| s.trim().is_empty())
}

fn number(value: &Value) -> Result<Option<f64>, &'static str> {
    if missing(value) {
        return Ok(None);
    }
    // The reviewed ArcGIS schema specifies native numbers. No guessing CSV types.
    value
        .as_f64()
        .filter(|n| n.is_finite())
        .map(Some)
        .ok_or("invalid-number")
}

fn identity(attributes: &Value) -> Option<String> {
    attributes["WellReportTrackingNumber"]
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 15 && id.bytes().all(|b| b.is_ascii_digit()))
        .map(|id| format!("TWDB-SDR-{id}"))
}

fn record(attributes: &Value) -> Result<Value, &'static str> {
    let id = identity(attributes).ok_or("invalid-report-id")?;
    let latitude = number(&attributes["LatitudeDD"])?.ok_or("missing-coordinates")?;
    let longitude = number(&attributes["LongitudeDD"])?.ok_or("missing-coordinates")?;
    // Broad Texas bounding box is an adapter guardrail, not a state polygon check.
    // Never swap axes, geocode, substitute geometry, or synthesize a zero point.
    if !(25.0..=37.0).contains(&latitude) || !(-107.0..=-93.0).contains(&longitude) {
        return Err("coordinates-outside-texas-guardrail");
    }
    let location_error = if missing(&attributes["LocationError"]) {
        None
    } else {
        Some(
            attributes["LocationError"]
                .as_i64()
                .filter(|n| i16::try_from(*n).is_ok())
                .ok_or("invalid-location-error")?,
        )
    };
    // No coded-value domain/definition was supplied in the reviewed metadata.
    // Nonzero values are held for review, never converted to metres of accuracy.
    if location_error.is_some_and(|n| n != 0) {
        return Err("location-error-requires-review");
    }
    let proposed_use = if missing(&attributes["ProposedUse"]) {
        None
    } else {
        Some(
            attributes["ProposedUse"]
                .as_str()
                .filter(|s| s.len() <= 50 && !s.chars().any(char::is_control))
                .ok_or("invalid-proposed-use")?
                .trim(),
        )
    };
    let depth = number(&attributes["BoreholeDepthFt"])?;
    if depth.is_some_and(|n| n < 0.0) {
        return Err("negative-borehole-depth");
    }
    let completion = if missing(&attributes["DateOfWellCompletion"]) {
        Value::Null
    } else {
        // ArcGIS esriFieldTypeDate: integer Unix milliseconds. Keep source precision;
        // do not infer a date-only local timezone or an observation timestamp.
        let ms = attributes["DateOfWellCompletion"]
            .as_i64()
            .filter(|n| (-62_135_596_800_000..=253_402_300_799_999).contains(n))
            .ok_or("invalid-completion-date")?;
        json!({"unixMilliseconds":ms,"timeZone":"UTC","precision":"source-timestamp"})
    };
    Ok(json!({
        "id":id,"source":"TWDB-SDR",
        "sourceRecordId":attributes["WellReportTrackingNumber"],
        "recordType":"submitted-driller-construction-report",
        "longitude":longitude,"latitude":latitude,"coordinateReference":"EPSG:4326",
        "locationError":location_error,"coordinateAccuracy":"unknown",
        "proposedUse":proposed_use,"boreholeDepth":{"value":depth,"unit":"ft"},
        "completedAt":completion,"activeWellStatus":"unverified",
        "ownershipStatus":"not-collected","waterSafetyStatus":"not-assessed"
    }))
}

/// Normalize one bounded synthetic ArcGIS-shaped response. Invalid rows are quarantined;
/// malformed envelopes, privacy/schema surprises and limits refuse the entire capture.
/// `synthetic` is an operator assertion, not an authenticity or rights attestation.
pub fn normalize(bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() > MAX_BYTES {
        return Err("SDR input byte limit".into());
    }
    // Do not include serde's source excerpts (which may contain unexpected private data).
    let input: Value = serde_json::from_slice(bytes).map_err(|_| "invalid SDR JSON")?;
    only_keys(
        &input,
        &[
            "schemaVersion",
            "kind",
            "synthetic",
            "sourceUrl",
            "retrievedAt",
            "response",
        ],
    )?;
    if input["schemaVersion"] != 1
        || input["kind"] != "rainfolk-texas-sdr-capture"
        || input["synthetic"] != true
        || input["sourceUrl"] != SOURCE
    {
        return Err(
            "SDR requires v1 synthetic capture at fixed source; live ingestion rights pending"
                .into(),
        );
    }
    if input["retrievedAt"].as_str().and_then(time_precision) != Some("instant") {
        return Err("SDR retrieval requires zoned timestamp".into());
    }
    let response = &input["response"];
    only_keys(
        response,
        &["spatialReference", "features", "exceededTransferLimit"],
    )?;
    only_keys(&response["spatialReference"], &["wkid"])?;
    if response["spatialReference"]["wkid"] != 4326
        || !response["exceededTransferLimit"].is_boolean()
    {
        return Err("SDR requires EPSG:4326 and explicit transfer-limit flag".into());
    }
    let rows = response["features"]
        .as_array()
        .ok_or("SDR features required")?;
    if rows.len() > MAX_ROWS {
        return Err("SDR row limit (200)".into());
    }
    // Projection is enforced before any row normalization, including rejected rows.
    for row in rows {
        only_keys(row, &["attributes"])?;
        only_keys(&row["attributes"], &FIELDS)?;
    }
    let input_hash = sha256(bytes);
    let mut quarantine = Vec::new();
    let mut grouped = BTreeMap::<String, Vec<(usize, Value)>>::new();
    let mut invalid_ids = BTreeSet::new();
    for (index, row) in rows.iter().enumerate() {
        let attributes = &row["attributes"];
        match record(attributes) {
            Ok(record) => grouped
                .entry(record["id"].as_str().unwrap().into())
                .or_default()
                .push((index, record)),
            Err(reason) => {
                let id = identity(attributes);
                if let Some(id) = &id {
                    invalid_ids.insert(id.clone());
                }
                quarantine.push(json!({"rowIndex":index,
                    "sourceId":id,"reason":reason,"captureSha256":input_hash}));
            }
        }
    }
    let mut records = Vec::new();
    let mut duplicates = 0;
    for (id, versions) in grouped {
        let first = &versions[0].1;
        if invalid_ids.contains(&id) || versions.iter().any(|(_, value)| value != first) {
            // No arrival-order winner for conflicting versions of one source report.
            for (index, _) in versions {
                quarantine.push(json!({"rowIndex":index,"sourceId":id,
                    "reason":"conflicting-report-id","captureSha256":input_hash}));
            }
            continue;
        }
        duplicates += versions.len() - 1;
        let mut normalized = first.clone();
        normalized["contentSha256"] = json!(sha256(&encoded(first)?));
        normalized["provenance"] = json!({"sourceUrl":SOURCE,"retrievedAt":input["retrievedAt"],
            "captureSha256":input_hash,"synthetic":true});
        records.push(normalized);
    }
    quarantine.sort_by_key(|q| q["rowIndex"].as_u64().unwrap());
    let payload = json!({"schemaVersion":1,"kind":"rainfolk-texas-sdr-construction-records",
        "synthetic":true,"records":records,"quarantine":quarantine,
        "coverage":{"complete":false,"stateCode":"48","observations":"not-requested",
            "exceededTransferLimit":response["exceededTransferLimit"],
            "description":"Synthetic adapter evidence only; construction reports do not verify active wells, ownership or water safety."},
        "accounting":{"inputRows":rows.len(),"acceptedRecords":records.len(),
            "identicalDuplicateRows":duplicates,"quarantinedRows":quarantine.len()}});
    let manifest = json!({"schemaVersion":1,"kind":"rainfolk-texas-sdr-manifest",
        "artifactEncoding":"serde-json-compact-sorted-keys-v1",
        "input":{"bytes":bytes.len(),"sha256":input_hash},
        "payload":artifact(&payload,1,records.len())?,
        "activation":"offline-review-only","rightsStatus":"confidential-report-exclusions-unresolved"});
    Ok(
        json!({"schemaVersion":1,"kind":"rainfolk-texas-sdr-candidate",
        "manifestSha256":sha256(&encoded(&manifest)?),"manifest":manifest,"payload":payload}),
    )
}
