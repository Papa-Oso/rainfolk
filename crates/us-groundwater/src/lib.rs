//! Offline core. Source identity is validated; capture provenance is an assertion
//! by the operator, not a cryptographic attestation of a USGS response.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub mod texas_sdr;

pub const MAX_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_SITES: usize = 2000;
pub const MAX_READINGS: usize = 16;
const API: &str = "https://api.waterdata.usgs.gov/ogcapi/v1/collections/";

fn array<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>, String> {
    value[key]
        .as_array()
        .ok_or_else(|| format!("expected array: {key}"))
}
fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 2048)
        .ok_or_else(|| format!("expected bounded text: {key}"))
}
fn site_id(id: &str) -> bool {
    id.strip_prefix("USGS-")
        .is_some_and(|s| (4..=15).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit()))
}

/// Numeric missingness never passes through a default zero.
pub fn numeric(value: &Value) -> Option<f64> {
    let number = match value {
        Value::String(s) if !s.trim().is_empty() => s.trim().parse::<f64>().ok(),
        Value::Number(n) => n.as_f64(),
        _ => None,
    };
    number.filter(|n| n.is_finite())
}

/// Preserve date-only and coarse field-visit dates without inventing midnight UTC.
pub fn time_precision(s: &str) -> Option<&'static str> {
    if !s.is_ascii() {
        return None;
    }
    let digit = |a: usize, b: usize| s.get(a..b)?.parse::<u32>().ok();
    let year = digit(0, 4)?;
    if year == 0 {
        return None;
    }
    if s.len() == 4 {
        return Some("year");
    }
    if s.get(4..5)? != "-" {
        return None;
    }
    let month = digit(5, 7)?;
    if !(1..=12).contains(&month) {
        return None;
    }
    if s.len() == 7 {
        return Some("month");
    }
    if s.get(7..8)? != "-" {
        return None;
    }
    let day = digit(8, 10)?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if day == 0 || day > days[(month - 1) as usize] {
        return None;
    }
    if s.len() == 10 {
        return Some("day");
    }
    if s.get(10..11)? != "T"
        || s.get(13..14)? != ":"
        || s.get(16..17)? != ":"
        || digit(11, 13)? > 23
        || digit(14, 16)? > 59
        || digit(17, 19)? > 59
    {
        return None;
    }
    let tail = s.get(19..)?;
    let zone = if let Some(fraction) = tail.strip_prefix('.') {
        let n = fraction.bytes().take_while(|b| b.is_ascii_digit()).count();
        if !(1..=9).contains(&n) {
            return None;
        }
        &fraction[n..]
    } else {
        tail
    };
    if zone != "Z" {
        if zone.len() != 6 || !matches!(&zone[0..1], "+" | "-") || &zone[3..4] != ":" {
            return None;
        }
        if zone[1..3].parse::<u32>().ok()? > 23 || zone[4..6].parse::<u32>().ok()? > 59 {
            return None;
        }
    }
    Some("instant")
}

fn acquisition(acq: &Value) -> Result<(&str, &Vec<Value>), String> {
    let collection = text(acq, "collection")?;
    if ![
        "monitoring-locations",
        "latest-continuous",
        "field-measurements",
    ]
    .contains(&collection)
    {
        return Err("unsupported collection".into());
    }
    let prefix = format!("{API}{collection}/items?");
    let url = text(acq, "url")?;
    if !url.starts_with(&prefix) || url.contains(['#', '\\', '\r', '\n']) {
        return Err("source URL outside fixed USGS collection".into());
    }
    if time_precision(text(acq, "retrievedAt")?) != Some("instant") {
        return Err("retrieval requires zoned timestamp".into());
    }
    let response = &acq["response"];
    if response["type"] != "FeatureCollection" {
        return Err("expected FeatureCollection".into());
    }
    let rows = array(response, "features")?;
    if rows.len() > MAX_SITES * MAX_READINGS {
        return Err("source feature limit".into());
    }
    Ok((collection, rows))
}

/// Validate captured OGC pages and export one record per authoritative U.S. well.
/// Partial queries remain partial, even if an individual page has no next link.
pub fn export(bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err("input byte limit".into());
    }
    let input: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if input["schemaVersion"] != 1 {
        return Err("unsupported capture schema".into());
    }
    let acquisitions = array(&input, "acquisitions")?;
    if acquisitions.is_empty() || acquisitions.len() > 12 {
        return Err("acquisition limit (1..12)".into());
    }
    let mut sites = BTreeMap::<String, Value>::new();
    let mut excluded = 0;
    let mut summaries = Vec::new();
    for acq in acquisitions {
        let (collection, rows) = acquisition(acq)?;
        summaries.push(json!({"collection":collection,"url":acq["url"],"retrievedAt":acq["retrievedAt"],"returned":rows.len(),"nextPageAvailable":acq["response"]["links"].as_array().is_some_and(|links| links.iter().any(|l|l["rel"]=="next"))}));
        if collection != "monitoring-locations" {
            continue;
        }
        for row in rows {
            let p = &row["properties"];
            if p["agency_code"] != "USGS"
                || p["country_code"] != "US"
                || p["site_type_code"] != "GW"
            {
                excluded += 1;
                continue;
            }
            let id = text(row, "id")?;
            if row["type"] != "Feature" || !site_id(id) || p["id"] != id {
                return Err("well identity mismatch".into());
            }
            let c = array(&row["geometry"], "coordinates")?;
            if row["geometry"]["type"] != "Point"
                || c.len() != 2
                || !c[0].as_f64().is_some_and(|n| (-180.0..=180.0).contains(&n))
                || !c[1].as_f64().is_some_and(|n| (-90.0..=90.0).contains(&n))
            {
                return Err("invalid authoritative coordinates".into());
            }
            let record = json!({"id":id,"name":p["monitoring_location_name"],"longitude":c[0],"latitude":c[1],"sourceUrl":format!("https://waterdata.usgs.gov/monitoring-location/{id}/"),"metadata":p,"metadataRetrievedAt":acq["retrievedAt"],"metadataSourceUrl":acq["url"],"readings":[]});
            if let Some(previous) = sites.insert(id.into(), record.clone()) {
                if previous != record {
                    return Err("conflicting duplicate well".into());
                }
            }
        }
    }
    if sites.len() > MAX_SITES {
        return Err("well feature limit".into());
    }
    let mut identities = BTreeSet::new();
    for acq in acquisitions {
        let (collection, rows) = acquisition(acq)?;
        if collection == "monitoring-locations" {
            continue;
        }
        for row in rows {
            let p = &row["properties"];
            let code = text(p, "parameter_code")?;
            // No stream stage, chemistry or guessed parameter semantics.
            let (reference, datum) = match code {
                "72019" => ("depth-below-land-surface", Value::Null),
                "62610" => ("water-level-elevation", json!("NGVD29")),
                "62611" => ("water-level-elevation", json!("NAVD88")),
                _ => {
                    excluded += 1;
                    continue;
                }
            };
            let id = text(p, "monitoring_location_id")?;
            let Some(site) = sites.get_mut(id) else {
                return Err(format!("observation without admitted well metadata: {id}"));
            };
            let observed = text(p, "time")?;
            let precision = time_precision(observed).ok_or("invalid observation date")?;
            if collection == "latest-continuous" && precision != "instant" {
                return Err("continuous observation requires zoned instant".into());
            }
            if row["type"] != "Feature"
                || !(p["value"].is_null() || p["value"].is_string() || p["value"].is_number())
            {
                return Err("invalid observation value".into());
            }
            let observation_id = text(row, "id")?;
            if observation_id.len() > 80
                || !observation_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                return Err("invalid observation identifier".into());
            }
            if !identities.insert(format!("{collection}:{observation_id}")) {
                return Err("duplicate observation identity".into());
            }
            let series_key = if collection == "latest-continuous" {
                "time_series_id"
            } else {
                "field_measurements_series_id"
            };
            text(p, series_key)?;
            let readings = site["readings"].as_array_mut().ok_or("missing readings")?;
            if readings.len() >= MAX_READINGS {
                return Err("readings per well limit".into());
            }
            readings.push(json!({"id":observation_id,"kind":if collection == "latest-continuous" {"continuous"} else {"discrete"},"parameterCode":code,"reference":reference,"datum":datum,"value":p["value"],"numericValue":numeric(&p["value"]),"unit":p["unit_of_measure"],"observedAt":observed,"timePrecision":precision,"retrievedAt":acq["retrievedAt"],"sourceUrl":format!("{API}{collection}/items/{observation_id}?f=json"),"queryUrl":acq["url"],"raw":p}));
        }
    }
    Ok(
        json!({"schemaVersion":1,"kind":"gev-us-groundwater-wells","mode":"snapshot","coverage":{"country":"US","complete":false,"description":"Bounded USGS monitoring-well sample; not all U.S. wells or universal current monitoring.","excludedSourceRows":excluded},"acquisitions":summaries,"records":sites.into_values().collect::<Vec<_>>()}),
    )
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn encoded(value: &Value) -> Result<Vec<u8>, String> {
    serde_json::to_vec(value).map_err(|error| error.to_string())
}

fn artifact(value: &Value, schema_version: u64, records: usize) -> Result<Value, String> {
    let bytes = encoded(value)?;
    Ok(json!({
        "schemaVersion": schema_version,
        "bytes": bytes.len(),
        "sha256": sha256(&bytes),
        "recordCount": records
    }))
}

fn split_pack(pack: &Value) -> Result<(Value, Value), String> {
    let records = array(pack, "records")?;
    let mut inventory = Vec::with_capacity(records.len());
    let mut observations = Vec::new();
    for site in records {
        let readings = array(site, "readings")?;
        inventory.push(json!({
            "id": site["id"],
            "name": site["name"],
            "longitude": site["longitude"],
            "latitude": site["latitude"],
            "sourceUrl": site["sourceUrl"],
            "metadata": site["metadata"],
            "metadataRetrievedAt": site["metadataRetrievedAt"],
            "metadataSourceUrl": site["metadataSourceUrl"]
        }));
        for reading in readings {
            observations.push(json!({ "wellId": site["id"], "reading": reading }));
        }
    }
    Ok((
        json!({
            "schemaVersion": 1,
            "kind": "gev-us-groundwater-inventory",
            "records": inventory
        }),
        json!({
            "schemaVersion": 1,
            "kind": "gev-us-groundwater-observations",
            "records": observations
        }),
    ))
}

/// Build a versioned, hashed producer bundle while preserving the browser's v1 pack.
/// Hashes cover compact serde_json bytes for each artifact, not the outer bundle.
pub fn bundle(bytes: &[u8]) -> Result<Value, String> {
    let pack = export(bytes)?;
    bundle_from_pack(bytes, pack)
}

/// Bounded inventory-only state pilot. No observations are inferred or fetched.
pub fn inventory_bundle(bytes: &[u8], state: &str) -> Result<Value, String> {
    if state.len() != 2 || !state.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("expected two-digit source state code".into());
    }
    if bytes.len() as u64 > MAX_BYTES {
        return Err("inventory pilot byte limit".into());
    }
    let input: Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let pages = array(&input, "acquisitions")?;
    if pages.is_empty() || pages.len() > 2 {
        return Err("inventory pilot page limit (1..2)".into());
    }
    let mut input_rows = 0;
    for page in pages {
        let (collection, rows) = acquisition(page)?;
        if collection != "monitoring-locations" || rows.len() > 1000 {
            return Err("inventory pilot requires metadata-only pages of at most 1000 rows".into());
        }
        input_rows += rows.len();
        for row in rows {
            let properties = &row["properties"];
            if properties["state_code"] != state
                || properties["agency_code"] != "USGS"
                || properties["country_code"] != "US"
                || properties["site_type_code"] != "GW"
            {
                return Err("inventory pilot source/state/type scope mismatch".into());
            }
        }
    }
    // Only absent geometry is quarantinable. Malformed non-null coordinates,
    // identities and scope still fail closed through the strict exporter.
    let mut filtered = input.clone();
    let mut quarantine = Vec::new();
    let mut source_ids = BTreeSet::new();
    for page in filtered["acquisitions"].as_array_mut().unwrap() {
        let url = page["url"].clone();
        let rows = page["response"]["features"].as_array_mut().unwrap();
        let mut retained = Vec::new();
        for row in rows.drain(..) {
            let id = text(&row, "id")?;
            if !site_id(id) || row["properties"]["id"] != id || row["type"] != "Feature" {
                return Err("well identity mismatch".into());
            }
            if !source_ids.insert(id.to_owned()) {
                return Err("duplicate source ID within inventory capture".into());
            }
            if row["geometry"].is_null() {
                quarantine.push(json!({"sourceId":id,"sourceUrl":url,"captureSha256":sha256(bytes),"reason":"missing-authoritative-geometry","rawFeature":row}));
            } else {
                retained.push(row);
            }
        }
        *rows = retained;
    }
    let mut pack = export(&encoded(&filtered)?)?;
    let accepted = array(&pack, "records")?.len();
    pack["coverage"]["stateCode"] = json!(state);
    pack["coverage"]["observationStatus"] = json!("not-requested");
    pack["coverage"]["accounting"] = json!({
        "inputRows": input_rows, "acceptedWells": accepted,
        "identicalDuplicateRows": 0, "rejected": 0, "quarantined": quarantine.len()
    });
    if !quarantine.is_empty() {
        pack["coverage"]["quarantine"] = json!(quarantine);
    }
    pack["coverage"]["description"] = json!(format!(
        "Bounded USGS groundwater monitoring-location inventory for state code {state}; partial sample, observations not requested."
    ));
    bundle_from_pack(bytes, pack)
}

fn bundle_from_pack(bytes: &[u8], pack: Value) -> Result<Value, String> {
    let (inventory, observations) = split_pack(&pack)?;
    let inventory_count = array(&inventory, "records")?.len();
    let observation_count = array(&observations, "records")?.len();
    let layer_count = array(&pack, "records")?.len();
    let manifest = json!({
        "schemaVersion": 1,
        "kind": "gev-us-groundwater-bundle-manifest",
        "artifactEncoding": "serde-json-compact-sorted-keys-v1",
        "producer": {
            "name": "gev-us-groundwater",
            "version": env!("CARGO_PKG_VERSION")
        },
        "input": {
            "bytes": bytes.len(),
            "sha256": sha256(bytes)
        },
        "artifacts": {
            "inventory": artifact(&inventory, 1, inventory_count)?,
            "observations": artifact(&observations, 1, observation_count)?,
            "layerPack": artifact(&pack, 1, layer_count)?
        },
        "activation": "validated-candidate-only"
    });
    let manifest_sha256 = sha256(&encoded(&manifest)?);
    let result = json!({
        "schemaVersion": 1,
        "kind": "gev-us-groundwater-artifact-bundle",
        "manifestSha256": manifest_sha256,
        "manifest": manifest,
        "inventory": inventory,
        "observations": observations,
        "layerPack": pack
    });
    validate_bundle(&result)?;
    Ok(result)
}

/// Check artifact bytes, counts and that split files still match the GEV v1 pack.
pub fn validate_bundle(bundle: &Value) -> Result<(), String> {
    if bundle["schemaVersion"] != 1
        || bundle["kind"] != "gev-us-groundwater-artifact-bundle"
        || bundle["manifest"]["schemaVersion"] != 1
        || bundle["manifest"]["kind"] != "gev-us-groundwater-bundle-manifest"
        || bundle["manifest"]["artifactEncoding"] != "serde-json-compact-sorted-keys-v1"
        || bundle["manifest"]["activation"] != "validated-candidate-only"
    {
        return Err("unsupported artifact bundle schema".into());
    }
    let manifest_sha256 = bundle["manifestSha256"]
        .as_str()
        .ok_or("missing manifest SHA-256")?;
    if manifest_sha256 != sha256(&encoded(&bundle["manifest"])?) {
        return Err("manifest integrity check failed".into());
    }
    let input_hash = text(&bundle["manifest"]["input"], "sha256")?;
    if input_hash.len() != 64 || !input_hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid input SHA-256".into());
    }
    if bundle["manifest"]["input"]["bytes"]
        .as_u64()
        .is_none_or(|bytes| bytes > MAX_BYTES)
    {
        return Err("invalid input byte count".into());
    }
    let pack = &bundle["layerPack"];
    if pack["schemaVersion"] != 1
        || pack["kind"] != "gev-us-groundwater-wells"
        || pack["mode"] != "snapshot"
        || pack["coverage"]["complete"] != false
    {
        return Err("invalid GEV v1 layer pack".into());
    }
    let (expected_inventory, expected_observations) = split_pack(pack)?;
    if pack["coverage"].get("quarantine").is_some() && !pack["coverage"]["quarantine"].is_array() {
        return Err("invalid quarantine list".into());
    }
    if let Some(quarantine) = pack["coverage"]["quarantine"].as_array() {
        let accounting = &pack["coverage"]["accounting"];
        if accounting["quarantined"] != quarantine.len()
            || accounting["acceptedWells"] != array(pack, "records")?.len()
            || accounting["inputRows"].as_u64()
                != Some((quarantine.len() + array(pack, "records")?.len()) as u64)
            || accounting["identicalDuplicateRows"] != 0
            || accounting["rejected"] != 0
        {
            return Err("quarantine accounting mismatch".into());
        }
        let mut ids: BTreeSet<String> = array(pack, "records")?
            .iter()
            .map(|r| r["id"].as_str().unwrap_or("").to_owned())
            .collect();
        for row in quarantine {
            let id = text(row, "sourceId")?;
            if !site_id(id)
                || !ids.insert(id.to_owned())
                || row["captureSha256"] != input_hash
                || row["rawFeature"]["id"] != id
                || row["rawFeature"]["properties"]["id"] != id
                || !row["rawFeature"]["geometry"].is_null()
                || row["reason"] != "missing-authoritative-geometry"
                || !text(row, "sourceUrl")?
                    .starts_with(&format!("{API}monitoring-locations/items?"))
            {
                return Err("invalid quarantine record".into());
            }
        }
    }
    if bundle["inventory"] != expected_inventory || bundle["observations"] != expected_observations
    {
        return Err("split artifacts do not match GEV v1 layer pack".into());
    }
    for (name, value, records) in [
        (
            "inventory",
            &bundle["inventory"],
            array(&bundle["inventory"], "records")?.len(),
        ),
        (
            "observations",
            &bundle["observations"],
            array(&bundle["observations"], "records")?.len(),
        ),
        ("layerPack", pack, array(pack, "records")?.len()),
    ] {
        let metadata = &bundle["manifest"]["artifacts"][name];
        let bytes = encoded(value)?;
        if metadata["schemaVersion"] != 1
            || metadata["bytes"].as_u64() != Some(bytes.len() as u64)
            || metadata["sha256"].as_str() != Some(sha256(&bytes).as_str())
            || metadata["recordCount"].as_u64() != Some(records as u64)
        {
            return Err(format!("artifact integrity check failed: {name}"));
        }
    }
    Ok(())
}

/// Return None for an identical validated input; callers may promote only Some(candidate).
/// This core deliberately writes no files, so a failed candidate cannot replace last-good.
pub fn prepare_activation(
    input: &[u8],
    last_good: Option<&Value>,
) -> Result<Option<Value>, String> {
    let candidate = bundle(input)?;
    if let Some(previous) = last_good {
        validate_bundle(previous)?;
        if previous["manifest"]["input"]["sha256"] == candidate["manifest"]["input"]["sha256"] {
            return Ok(None);
        }
    }
    Ok(Some(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;
    const INPUT: &[u8] = include_bytes!("../../../fixtures/us-groundwater/v1/capture.json");
    #[test]
    fn missing_zero_negative() {
        for value in [
            json!(null),
            json!(""),
            json!(" "),
            json!("Ice"),
            json!("NaN"),
            json!(false),
        ] {
            assert_eq!(numeric(&value), None);
        }
        assert_eq!(numeric(&json!("0")), Some(0.0));
        assert_eq!(numeric(&json!("-64.73")), Some(-64.73));
    }
    #[test]
    fn dates_keep_precision() {
        assert_eq!(time_precision("1939-11-07"), Some("day"));
        assert_eq!(time_precision("2026-10"), Some("month"));
        for bad in [
            "2025-02-29",
            "2026-10-24T01:00:00",
            "é234-01-01",
            "2026-10-24T01:00:00+é:00",
        ] {
            assert_eq!(time_precision(bad), None);
        }
    }
    #[test]
    fn authoritative_fixture_matches_contract() {
        let actual = export(INPUT).unwrap();
        let expected: Value = serde_json::from_slice(include_bytes!(
            "../../../fixtures/us-groundwater/v1/expected-pack.json"
        ))
        .unwrap();
        assert_eq!(actual, expected);
    }
    #[test]
    fn source_and_join_refusals() {
        let input: Value = serde_json::from_slice(INPUT).unwrap();
        let mut bad = input.clone();
        bad["acquisitions"][0]["url"] = json!("https://example.com/");
        assert!(export(&serde_json::to_vec(&bad).unwrap()).is_err());
        let mut bad = input.clone();
        bad["acquisitions"][0]["response"]["features"][0]["geometry"]["coordinates"][0] =
            json!(null);
        assert!(export(&serde_json::to_vec(&bad).unwrap()).is_err());
        let mut bad = input;
        bad["acquisitions"][1]["response"]["features"][0]["properties"]["monitoring_location_id"] =
            json!("USGS-123456789");
        assert!(export(&serde_json::to_vec(&bad).unwrap()).is_err());
    }

    #[test]
    fn malformed_monitoring_location_records_are_refused() {
        let input: Value = serde_json::from_slice(INPUT).unwrap();
        let mut bad_type = input.clone();
        bad_type["acquisitions"][0]["response"]["features"][0]["type"] = json!("Collection");
        assert!(export(&serde_json::to_vec(&bad_type).unwrap()).is_err());

        let mut bad_identity = input.clone();
        bad_identity["acquisitions"][0]["response"]["features"][0]["id"] = json!("USGS-123456789");
        assert!(export(&serde_json::to_vec(&bad_identity).unwrap()).is_err());

        let mut out_of_range = input;
        out_of_range["acquisitions"][0]["response"]["features"][0]["geometry"]["coordinates"][1] =
            json!(91.0);
        assert!(export(&serde_json::to_vec(&out_of_range).unwrap()).is_err());
    }

    #[test]
    fn duplicate_source_ids_only_coalesce_identical_wells_and_refuse_duplicate_observations() {
        let input: Value = serde_json::from_slice(INPUT).unwrap();

        let mut identical_well = input.clone();
        let rows = identical_well["acquisitions"][0]["response"]["features"]
            .as_array_mut()
            .unwrap();
        rows.push(rows[0].clone());
        let pack = export(&serde_json::to_vec(&identical_well).unwrap()).unwrap();
        assert_eq!(pack["records"].as_array().unwrap().len(), 6);

        let mut conflicting_well = input.clone();
        let rows = conflicting_well["acquisitions"][0]["response"]["features"]
            .as_array_mut()
            .unwrap();
        let mut duplicate = rows[0].clone();
        duplicate["properties"]["monitoring_location_name"] = json!("conflicting source row");
        rows.push(duplicate);
        assert!(export(&serde_json::to_vec(&conflicting_well).unwrap()).is_err());

        let mut duplicate_observation = input;
        let rows = duplicate_observation["acquisitions"][1]["response"]["features"]
            .as_array_mut()
            .unwrap();
        rows.push(rows[0].clone());
        assert!(export(&serde_json::to_vec(&duplicate_observation).unwrap()).is_err());
    }

    #[test]
    fn signed_values_and_unknown_raw_unit_and_datum_keep_source_ids() {
        let mut input: Value = serde_json::from_slice(INPUT).unwrap();
        let feature = &mut input["acquisitions"][1]["response"]["features"][2];
        let source_observation_id = feature["id"].clone();
        let source_well_id = feature["properties"]["monitoring_location_id"].clone();
        feature["properties"]["value"] = json!("-17.25");
        feature["properties"]["unit_of_measure"] = json!("unit-not-verified");
        feature["properties"]["vertical_datum"] = json!("datum-not-verified");

        let pack = export(&serde_json::to_vec(&input).unwrap()).unwrap();
        let well = pack["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|well| well["id"] == source_well_id)
            .unwrap();
        let reading = well["readings"].as_array().unwrap().first().unwrap();

        assert_eq!(well["id"], source_well_id);
        assert_eq!(reading["id"], source_observation_id);
        assert_eq!(reading["raw"]["monitoring_location_id"], source_well_id);
        assert_eq!(reading["value"], "-17.25");
        assert_eq!(reading["numericValue"], -17.25);
        assert_eq!(reading["unit"], "unit-not-verified");
        assert_eq!(reading["raw"]["unit_of_measure"], "unit-not-verified");
        assert_eq!(reading["raw"]["vertical_datum"], "datum-not-verified");
        assert_eq!(reading["reference"], "depth-below-land-surface");
        assert!(reading["datum"].is_null());
    }

    #[test]
    fn bundle_splits_inventory_and_observations_and_keeps_v1_contract() {
        let value = bundle(INPUT).unwrap();
        validate_bundle(&value).unwrap();
        assert_eq!(value["layerPack"], export(INPUT).unwrap());
        assert_eq!(value["inventory"]["records"].as_array().unwrap().len(), 6);
        assert_eq!(
            value["observations"]["records"].as_array().unwrap().len(),
            6
        );
        assert!(value["inventory"]["records"][0].get("readings").is_none());
        assert!(value["observations"]["records"][0].get("wellId").is_some());
        assert_eq!(
            value["manifest"]["artifacts"]["inventory"]["schemaVersion"],
            1
        );
        assert_eq!(
            value["manifest"]["artifacts"]["observations"]["schemaVersion"],
            1
        );
    }

    #[test]
    fn bundle_is_deterministic_and_records_input_and_artifact_hashes() {
        let first = bundle(INPUT).unwrap();
        let second = bundle(INPUT).unwrap();
        assert_eq!(first, second);
        let inventory_bytes = encoded(&first["inventory"]).unwrap();
        assert_eq!(
            first["manifest"]["artifacts"]["inventory"]["sha256"],
            sha256(&inventory_bytes)
        );
        assert_eq!(first["manifest"]["input"]["sha256"], sha256(INPUT));
        assert_eq!(
            first["manifestSha256"],
            sha256(&encoded(&first["manifest"]).unwrap())
        );
    }

    #[test]
    fn sha256_uses_standard_digest() {
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn validation_and_activation_refuse_bad_candidates_and_noop_identical_input() {
        let previous = bundle(INPUT).unwrap();
        assert!(
            prepare_activation(INPUT, Some(&previous))
                .unwrap()
                .is_none()
        );
        let changed_input = [INPUT, b" "].concat();
        assert!(
            prepare_activation(&changed_input, Some(&previous))
                .unwrap()
                .is_some()
        );
        let mut tampered = previous.clone();
        tampered["observations"]["records"][0]["reading"]["value"] = json!(0);
        assert!(validate_bundle(&tampered).is_err());
        let mut tampered_manifest = previous.clone();
        tampered_manifest["manifest"]["input"]["sha256"] = json!("0".repeat(64));
        assert!(validate_bundle(&tampered_manifest).is_err());
        let before = previous.clone();
        let mut bad_input: Value = serde_json::from_slice(INPUT).unwrap();
        bad_input["acquisitions"][0]["url"] = json!("https://example.com/");
        assert!(
            prepare_activation(&serde_json::to_vec(&bad_input).unwrap(), Some(&previous)).is_err()
        );
        assert_eq!(previous, before);
        assert!(prepare_activation(INPUT, None).unwrap().is_some());
    }

    #[test]
    fn validation_refuses_tampered_artifact_hash_before_returning_a_candidate() {
        let mut candidate = bundle(INPUT).unwrap();
        candidate["manifest"]["artifacts"]["observations"]["sha256"] = json!("0".repeat(64));
        candidate["manifestSha256"] = json!(sha256(&encoded(&candidate["manifest"]).unwrap()));

        let valid_last_good = bundle(INPUT).unwrap();
        let before = valid_last_good.clone();
        assert!(validate_bundle(&candidate).is_err());
        assert!(prepare_activation(INPUT, Some(&candidate)).is_err());
        assert_eq!(valid_last_good, before);
    }

    fn inventory_pilot_input() -> Value {
        let mut input: Value = serde_json::from_slice(INPUT).unwrap();
        input["acquisitions"].as_array_mut().unwrap().truncate(1);
        input["acquisitions"][0]["response"]["features"]
            .as_array_mut()
            .unwrap()
            .retain(|row| row["properties"]["state_code"] == "36");
        input
    }

    #[test]
    fn inventory_pilot_preserves_captured_ids_metadata_and_no_observation_claim() {
        let input = inventory_pilot_input();
        let bytes = serde_json::to_vec(&input).unwrap();
        let candidate = inventory_bundle(&bytes, "36").unwrap();
        validate_bundle(&candidate).unwrap();
        assert_eq!(candidate, inventory_bundle(&bytes, "36").unwrap());
        assert_eq!(
            candidate["inventory"]["records"].as_array().unwrap().len(),
            1
        );
        assert_eq!(
            candidate["inventory"]["records"][0]["id"],
            "USGS-422702079005101"
        );
        assert_eq!(
            candidate["inventory"]["records"][0]["metadata"],
            input["acquisitions"][0]["response"]["features"][0]["properties"]
        );
        assert_eq!(candidate["observations"]["records"], json!([]));
        assert_eq!(candidate["layerPack"]["coverage"]["complete"], false);
        assert_eq!(
            candidate["layerPack"]["coverage"]["observationStatus"],
            "not-requested"
        );
        assert_eq!(
            candidate["layerPack"]["coverage"]["accounting"]["inputRows"],
            1
        );
    }

    #[test]
    fn inventory_pilot_refuses_scope_pages_bytes_observations_and_bad_coordinates() {
        let input = inventory_pilot_input();
        let bytes = serde_json::to_vec(&input).unwrap();
        assert!(inventory_bundle(&bytes, "22").is_err());
        assert!(inventory_bundle(&bytes, "036").is_err());
        assert!(inventory_bundle(&vec![b' '; MAX_BYTES as usize + 1], "36").is_err());
        let mut too_many_rows = input.clone();
        let row = too_many_rows["acquisitions"][0]["response"]["features"][0].clone();
        too_many_rows["acquisitions"][0]["response"]["features"] = json!(vec![row; 1001]);
        assert!(inventory_bundle(&serde_json::to_vec(&too_many_rows).unwrap(), "36").is_err());
        let mut pages = input.clone();
        pages["acquisitions"] = json!(vec![input["acquisitions"][0].clone(); 3]);
        assert!(inventory_bundle(&serde_json::to_vec(&pages).unwrap(), "36").is_err());
        let mut wrong_collection = input.clone();
        wrong_collection["acquisitions"][0]["collection"] = json!("latest-continuous");
        assert!(inventory_bundle(&serde_json::to_vec(&wrong_collection).unwrap(), "36").is_err());
        let mut bad = input;
        bad["acquisitions"][0]["response"]["features"][0]["geometry"]["coordinates"][0] =
            Value::Null;
        assert!(inventory_bundle(&serde_json::to_vec(&bad).unwrap(), "36").is_err());
    }
}
