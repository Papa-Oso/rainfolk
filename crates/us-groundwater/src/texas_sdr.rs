//! Bounded Texas SDR construction reports. No transport or index activation.
//! Reuses the producer's compact JSON encoding, SHA-256 and artifact descriptors.
use super::{artifact, encoded, sha256, time_precision};
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
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

/// Exact minimum-field request for the separately authorized one-page public sample.
pub fn public_query_url() -> String {
    format!(
        "{SOURCE}/query?f=json&where=1%3D1&outFields={}&returnGeometry=false&orderByFields=WellReportTrackingNumber&resultRecordCount=200&resultOffset=0",
        FIELDS.join("%2C")
    )
}

/// Fixed first page omitting the date field that failed in the retained public pilot.
pub fn public_batch_query_url() -> String {
    let fields: Vec<_> = FIELDS
        .iter()
        .copied()
        .filter(|field| *field != "DateOfWellCompletion")
        .collect();
    format!(
        "{SOURCE}/query?f=pjson&where=1%3D1&outFields={}&returnGeometry=false&orderByFields=ObjectId%20ASC&resultRecordCount=200&resultOffset=0",
        fields.join("%2C")
    )
}

/// Keyset page on the source's non-null OID. OIDs are paging metadata, not well IDs.
pub fn public_job_query_url(after: u64, limit: usize) -> Result<String, String> {
    if after > i32::MAX as u64 || !(1..=MAX_ROWS).contains(&limit) {
        return Err("invalid SDR keyset bounds".into());
    }
    let mut fields: Vec<_> = FIELDS
        .iter()
        .copied()
        .filter(|f| *f != "DateOfWellCompletion")
        .collect();
    fields.push("ObjectId");
    Ok(format!(
        "{SOURCE}/query?f=pjson&where=ObjectId%3E{after}&outFields={}&returnGeometry=false&orderByFields=ObjectId%20ASC&resultRecordCount={limit}",
        fields.join("%2C")
    ))
}

pub fn job_projection(url: &str) -> Option<(u64, usize)> {
    let tail = url.strip_prefix(&format!("{SOURCE}/query?f=pjson&where=ObjectId%3E"))?;
    let (after, _) = tail.split_once("&outFields=")?;
    let after = after.parse::<u64>().ok()?;
    let (_, limit) = tail.rsplit_once("&resultRecordCount=")?;
    let limit = limit.parse::<usize>().ok()?;
    (public_job_query_url(after, limit).ok()? == url).then_some((after, limit))
}

/// Admit only fixed first pages, strict keyset pages or a one-row projection.
fn public_projection(url: &str) -> Option<(Vec<&'static str>, usize)> {
    if url == public_query_url() {
        return Some((FIELDS.to_vec(), MAX_ROWS));
    }
    if url == public_batch_query_url() {
        return Some((
            FIELDS
                .iter()
                .copied()
                .filter(|field| *field != "DateOfWellCompletion")
                .collect(),
            MAX_ROWS,
        ));
    }
    if let Some((_, limit)) = job_projection(url) {
        return Some((
            FIELDS
                .iter()
                .copied()
                .filter(|f| *f != "DateOfWellCompletion")
                .collect(),
            limit,
        ));
    }
    if url.len() > 1024 {
        return None;
    }
    let prefix = format!("{SOURCE}/query?f=pjson&where=1%3D1&outFields=");
    let fields = url
        .strip_prefix(&prefix)?
        .strip_suffix("&returnGeometry=false&resultRecordCount=1")?;
    let mut selected = Vec::new();
    for name in fields.split("%2C").take(FIELDS.len() + 1) {
        let field = FIELDS.iter().find(|field| **field == name)?;
        if selected.contains(field) || selected.len() == FIELDS.len() {
            return None;
        }
        selected.push(*field);
    }
    if selected.first() != Some(&FIELDS[0]) {
        return None;
    }
    Some((selected, 1))
}

// Use the already-locked Serde JSON deserializer to inspect every member before
// Value's map representation could erase duplicate keys. This bounded preflight
// retains no values and uses the parser's existing syntax/recursion limits.
struct UniqueMembers;

impl<'de> Deserialize<'de> for UniqueMembers {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueMembers)
    }
}

impl<'de> Visitor<'de> for UniqueMembers {
    type Value = Self;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("JSON with unique object members")
    }

    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self, E> {
        Ok(self)
    }
    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self, E> {
        Ok(self)
    }
    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self, E> {
        Ok(self)
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self, E> {
        Ok(self)
    }
    fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Self, E> {
        Ok(self)
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<Self, E> {
        Ok(self)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut array: A) -> Result<Self, A::Error> {
        while array.next_element::<UniqueMembers>()?.is_some() {}
        Ok(self)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<Self, A::Error> {
        let mut keys = BTreeSet::new();
        while let Some(key) = object.next_key::<String>()? {
            if !keys.insert(key) {
                return Err(serde::de::Error::custom("duplicate JSON member"));
            }
            object.next_value::<UniqueMembers>()?;
        }
        Ok(self)
    }
}

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
        // IEEE signed zero compares equal but serializes differently. Canonicalize
        // before duplicate reconciliation and hashes so arrival order cannot win.
        .map(|n| Some(if n == 0.0 { 0.0 } else { n }))
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
    let reported_latitude = number(&attributes["LatitudeDD"])?;
    let reported_longitude = number(&attributes["LongitudeDD"])?;
    // Broad Texas bounding box is an adapter guardrail, not a state polygon check.
    // Never swap axes, geocode, substitute geometry, or synthesize a zero point.
    let (latitude, longitude, coordinate_quality) = match (reported_latitude, reported_longitude) {
        (Some(lat), Some(lon))
            if (25.0..=37.0).contains(&lat) && (-107.0..=-93.0).contains(&lon) =>
        {
            (Some(lat), Some(lon), "unverified-source-coordinates")
        }
        (Some(_), Some(_)) => (None, None, "coordinates-outside-texas-guardrail"),
        _ => (None, None, "missing-coordinates"),
    };
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
    // No code changes retention or eligibility: its meaning is undocumented.
    // Valid coordinates remain eligible with an explicit unverified warning.
    let map_eligible = latitude.is_some();
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
        "sourceRecordLookup":{"url":"https://www3.twdb.texas.gov/apps/WaterDataInteractive/GroundwaterDataViewer/?map=sdr","trackingNumber":attributes["WellReportTrackingNumber"]},
        "recordType":"submitted-driller-construction-report",
        "longitude":longitude,"latitude":latitude,"coordinateReference":null,
        "serviceCoordinateReference":"EPSG:4326","coordinateVerification":"unverified",
        "sourceCoordinates":{"latitude":attributes["LatitudeDD"],"longitude":attributes["LongitudeDD"]},
        "coordinateQuality":coordinate_quality,
        "mapEligible":map_eligible,
        "mapSuppressionReason":if map_eligible {Value::Null} else {json!(coordinate_quality)},
        "locationQualityWarning":if location_error.is_some() {"uninterpreted-location-error; coordinates-unverified"} else {"accuracy-unknown; coordinates-unverified"},
        "locationError":location_error,"coordinateAccuracy":"unknown",
        "proposedUse":proposed_use,"boreholeDepth":{"value":depth,"unit":"ft"},
        "completedAt":completion,"activeWellStatus":"unverified",
        "ownershipStatus":"not-collected","waterSafetyStatus":"not-assessed"
    }))
}

/// Normalize one bounded synthetic or explicitly public ArcGIS-shaped response. Invalid rows are quarantined;
/// malformed envelopes, privacy/schema surprises and limits refuse the entire capture.
/// `synthetic` is an operator assertion, not an authenticity or rights attestation.
pub fn normalize(bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() > MAX_BYTES {
        return Err("SDR input byte limit".into());
    }
    // Do not include serde's source excerpts (which may contain unexpected private data).
    serde_json::from_slice::<UniqueMembers>(bytes)
        .map_err(|_| "invalid SDR JSON or duplicate member")?;
    let input: Value = serde_json::from_slice(bytes).map_err(|_| "invalid SDR JSON")?;
    let public = input["kind"] == "rainfolk-texas-sdr-public-capture";
    only_keys(
        &input,
        if public {
            &[
                "schemaVersion",
                "kind",
                "synthetic",
                "sourceUrl",
                "retrievedAt",
                "response",
                "queryUrl",
                "sourceResponseSha256",
                "sourceSchemaSha256",
                "fieldLimitations",
                "fieldLimitationsEvidenceSha256",
                "pagination",
            ]
        } else {
            &[
                "schemaVersion",
                "kind",
                "synthetic",
                "sourceUrl",
                "retrievedAt",
                "response",
            ]
        },
    )?;
    if input["schemaVersion"] != 1
        || (!public && input["kind"] != "rainfolk-texas-sdr-capture")
        || input["synthetic"] != !public
        || input["sourceUrl"] != SOURCE
    {
        return Err("SDR requires explicit v1 synthetic/public capture at fixed source".into());
    }
    let projection = if public {
        let projection = input["queryUrl"]
            .as_str()
            .and_then(public_projection)
            .ok_or("SDR public capture requires bounded whitelist query")?;
        for field in ["sourceResponseSha256", "sourceSchemaSha256"] {
            if input[field]
                .as_str()
                .is_none_or(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err("SDR public capture requires response/schema hashes".into());
            }
        }
        Some(projection)
    } else {
        None
    };
    let job = input["queryUrl"].as_str().and_then(job_projection);
    if public
        && (input["queryUrl"] == public_batch_query_url() || job.is_some())
        && input["fieldLimitations"] != json!({"DateOfWellCompletion":"source-query-api-400"})
    {
        return Err("SDR batch requires date-omission limitation provenance".into());
    }
    if let Some(limitations) = input.get("fieldLimitations") {
        only_keys(limitations, &FIELDS)?;
        let (selected, _) = projection
            .as_ref()
            .ok_or("field limitations require public capture")?;
        if limitations.as_object().unwrap().is_empty()
            || limitations
                .as_object()
                .unwrap()
                .iter()
                .any(|(field, reason)| {
                    selected.contains(&field.as_str()) || reason != "source-query-api-400"
                })
            || input["fieldLimitationsEvidenceSha256"]
                .as_str()
                .is_none_or(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err("invalid unrequested-field limitation provenance".into());
        }
    } else if input.get("fieldLimitationsEvidenceSha256").is_some() {
        return Err("field limitation evidence requires limitations".into());
    }
    if input["retrievedAt"].as_str().and_then(time_precision) != Some("instant") {
        return Err("SDR retrieval requires zoned timestamp".into());
    }
    let response = &input["response"];
    only_keys(
        response,
        &[
            "spatialReference",
            "spatialReferenceBasis",
            "features",
            "exceededTransferLimit",
        ],
    )?;
    only_keys(&response["spatialReference"], &["wkid"])?;
    if response["spatialReference"]["wkid"] != 4326
        || !response["exceededTransferLimit"].is_boolean()
    {
        return Err("SDR requires EPSG:4326 and explicit transfer-limit flag".into());
    }
    if public
        && !matches!(
            response["spatialReferenceBasis"].as_str(),
            Some(
                "source-response; attributes-unverified"
                    | "validated-layer-schema; attributes-unverified"
            )
        )
    {
        return Err("SDR public capture requires explicit service CRS provenance basis".into());
    }
    let rows = response["features"]
        .as_array()
        .ok_or("SDR features required")?;
    if rows.len() > MAX_ROWS {
        return Err("SDR row limit (200)".into());
    }
    if projection
        .as_ref()
        .is_some_and(|(_, limit)| rows.len() > *limit)
    {
        return Err("SDR capture exceeds declared query row limit".into());
    }
    if let Some((after, _)) = job {
        only_keys(&input["pagination"], &["afterObjectId", "objectIds"])?;
        let ids = input["pagination"]["objectIds"]
            .as_array()
            .ok_or("SDR paging OIDs required")?;
        if input["pagination"]["afterObjectId"].as_u64() != Some(after) || ids.len() != rows.len() {
            return Err("SDR paging provenance mismatch".into());
        }
        let mut previous = after;
        for id in ids {
            let id = id
                .as_u64()
                .filter(|n| *n > previous && *n <= i32::MAX as u64)
                .ok_or("SDR paging OIDs not strictly increasing")?;
            previous = id;
        }
        if ids.is_empty() && response["exceededTransferLimit"] == true {
            return Err("SDR paging made no progress".into());
        }
    } else if input.get("pagination").is_some() {
        return Err("SDR pagination requires exact keyset query".into());
    }
    // Projection is enforced before any row normalization, including rejected rows.
    for row in rows {
        only_keys(row, &["attributes"])?;
        only_keys(&row["attributes"], &FIELDS)?;
        if let Some((selected, _)) = &projection {
            only_keys(&row["attributes"], selected)?;
        }
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
        // Compare canonical bytes, not Value float equality: retained raw source
        // coordinates can distinguish -0.0 from 0.0 even when JSON values compare equal.
        let first_bytes = encoded(first)?;
        let conflicting = versions
            .iter()
            .map(|(_, value)| encoded(value))
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .any(|value| *value != first_bytes);
        if invalid_ids.contains(&id) || conflicting {
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
            "captureSha256":input_hash,"synthetic":!public});
        if public {
            for field in ["queryUrl", "sourceResponseSha256", "sourceSchemaSha256"] {
                normalized["provenance"][field] = input[field].clone();
            }
            normalized["provenance"]["serviceSpatialReferenceBasis"] =
                response["spatialReferenceBasis"].clone();
            let (selected, _) = projection.as_ref().unwrap();
            normalized["provenance"]["requestedFields"] = json!(selected);
            normalized["provenance"]["unrequestedFields"] = json!(
                FIELDS
                    .iter()
                    .filter(|field| !selected.contains(field))
                    .collect::<Vec<_>>()
            );
            normalized["provenance"]["unrequestedFieldStatus"] =
                json!("unknown; not requested in bounded source projection");
            if let Some(limitations) = input.get("fieldLimitations") {
                normalized["provenance"]["fieldLimitations"] = limitations.clone();
                normalized["provenance"]["fieldLimitationsEvidenceSha256"] =
                    input["fieldLimitationsEvidenceSha256"].clone();
            }
        }
        records.push(normalized);
    }
    quarantine.sort_by_key(|q| q["rowIndex"].as_u64().unwrap());
    let mapped = records.iter().filter(|r| r["mapEligible"] == true).count();
    let mut payload = json!({"schemaVersion":1,"kind":"rainfolk-texas-sdr-construction-records",
        "synthetic":!public,"records":records,"quarantine":quarantine,
        "coverage":{"complete":false,"stateCode":"48","observations":"not-requested",
            "exceededTransferLimit":response["exceededTransferLimit"],
            "description":if public {"Bounded public SDR construction-report sample; export confidentiality filtering unverified; coordinates unverified; no active-well, ownership or water-safety inference."} else {"Synthetic adapter evidence only; construction reports do not verify active wells, ownership or water safety."}},
        "accounting":{"inputRows":rows.len(),"acceptedRecords":records.len(),
            "identicalDuplicateRows":duplicates,"quarantinedRows":quarantine.len(),
            "mapEligibleRecords":mapped,"nonmappedRecords":records.len()-mapped}});
    if job.is_some() {
        payload["pagination"] = input["pagination"].clone();
    }
    let manifest = json!({"schemaVersion":1,"kind":"rainfolk-texas-sdr-manifest",
        "artifactEncoding":"serde-json-compact-sorted-keys-v1",
        "input":{"bytes":bytes.len(),"sha256":input_hash},
        "payload":artifact(&payload,1,records.len())?,
        "activation":if public {"local-review-only"} else {"offline-review-only"},
        "rightsStatus":if public {"bounded-public-workflow-evidence-export-filter-unverified"} else {"synthetic-evidence-only"}});
    Ok(
        json!({"schemaVersion":1,"kind":"rainfolk-texas-sdr-candidate",
        "manifestSha256":sha256(&encoded(&manifest)?),"manifest":manifest,"payload":payload}),
    )
}
