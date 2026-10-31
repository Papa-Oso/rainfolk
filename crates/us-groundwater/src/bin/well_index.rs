//! Offline spatial publication of pinned, validated Rust candidate bundles.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::Path};
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

// Shared across every immutable artifact in one index invocation.
struct Budget {
    started: std::time::Instant,
    bytes: std::cell::Cell<u64>,
    seconds: u64,
}
impl Budget {
    fn new() -> Self {
        Self {
            started: std::time::Instant::now(),
            bytes: std::cell::Cell::new(0),
            seconds: 300,
        }
    }
    fn check(&self) -> Result<(), Box<dyn std::error::Error>> {
        if self.started.elapsed().as_secs() >= self.seconds {
            return Err(format!("index {}-second deadline", self.seconds).into());
        }
        Ok(())
    }
    fn charge(&self, bytes: usize) -> Result<(), Box<dyn std::error::Error>> {
        self.check()?;
        let next = self
            .bytes
            .get()
            .checked_add(bytes as u64)
            .ok_or("index output overflow")?;
        if next > 16 * 1024 * 1024 * 1024 {
            return Err("index 16-GiB logical output cap".into());
        }
        self.bytes.set(next);
        Ok(())
    }
}

// One writer owns both immutable publication and the mutable index pointer.
// A process crash leaves the lock behind: continuation fails closed until an
// operator has established that no writer survives. Never guess from its age.
struct IndexWriterLock {
    path: std::path::PathBuf,
    file: Option<fs::File>,
}
impl IndexWriterLock {
    fn acquire(out: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        fs::create_dir_all(out)?;
        let path = out.join(".well-index.lock");
        let file = fs::OpenOptions::new().write(true).create_new(true).open(&path)
            .map_err(|e| format!("index writer lock unavailable; do not remove without verifying no active writer: {e}"))?;
        let mut lock = Self {
            path,
            file: Some(file),
        };
        std::io::Write::write_all(
            lock.file.as_mut().unwrap(),
            format!("{}\n", std::process::id()).as_bytes(),
        )?;
        lock.file.as_ref().unwrap().sync_all()?;
        Ok(lock)
    }
}
impl Drop for IndexWriterLock {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

fn coverage_description(pack: &Value) -> String {
    if pack["acquisitions"].as_array().is_some_and(|a| {
        a.iter()
            .any(|a| a["collection"] == "usgwd-historical-baseline")
    }) {
        return "Historical source catalog records; physical-well identity unresolved. Locations unverified; approximate datum conversion. No water-level observations or verified current activity/water safety.".into();
    }
    let readings = pack["records"].as_array().is_some_and(|records| {
        records
            .iter()
            .any(|r| r["readings"].as_array().is_some_and(|v| !v.is_empty()))
    });
    let observation_query = pack["acquisitions"].as_array().is_some_and(|acquisitions| {
        acquisitions.iter().any(|a| {
            matches!(
                a["collection"].as_str(),
                Some("latest-continuous" | "field-measurements")
            )
        })
    });
    let inventory_only = pack["acquisitions"].as_array().is_some_and(|acquisitions| {
        !acquisitions.is_empty()
            && acquisitions
                .iter()
                .all(|a| a["collection"] == "monitoring-locations")
    });
    let scope = if readings {
        "Historical observations are included; consult each reading's measurement and retrieval dates."
    } else if observation_query {
        "Observation collections were queried; this pack contains no readings."
    } else if inventory_only {
        "Inventory metadata only; no observation collection appears in this artifact's acquisition metadata."
    } else {
        "Observation acquisition scope is unknown; consult provenance."
    };
    format!(
        "Partial collected USGS well records; bounded spatial subset. Absent dots do not establish absence of wells. {scope}"
    )
}
// Never open a referenced content-addressed file for writing. Interrupted
// staging leaves only an unreferenced temporary file; publication is a rename.
fn write_immutable(
    out: &Path,
    budget: &Budget,
    name: &str,
    bytes: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    budget.charge(bytes.len())?;
    let target = out.join(name);
    if target.exists() {
        if hash(&fs::read(&target)?) != hash(bytes) {
            return Err("existing count artifact changed".into());
        }
        return Ok(());
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let stage = out.join(format!("{name}.{}.{}.pending", std::process::id(), nonce));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stage)?;
    std::io::Write::write_all(&mut file, bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(stage, target)?;
    Ok(())
}
type DetailRow = (Value, Vec<Value>);

fn publish_count_file(
    out: &Path,
    budget: &Budget,
    value: &Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("partitioned aggregate byte cap".into());
    }
    let digest = hash(&bytes);
    let name = format!("{digest}.counts.json");
    write_immutable(out, budget, &name, &bytes)?;
    Ok(json!({"url":name,"sha256":digest,"bytes":bytes.len()}))
}

// Keep the existing cell representation, but publish a small global prefix and
// complete fine subtrees anchored to its existing two-degree parent cells.
fn publish_partitioned_counts(
    out: &Path,
    budget: &Budget,
    levels: &[Value],
    total: usize,
) -> Result<Value, Box<dyn std::error::Error>> {
    if levels.len() != 6 {
        return Err("count level count".into());
    }
    let overview = publish_count_file(
        out,
        budget,
        &json!({"schemaVersion":1,"kind":"gev-well-count-grids","centerMethod":"record-mean","total":total,"levels":&levels[..4]}),
    )?;
    let parent_key = |bounds: &Value| -> Result<(i32, i32), Box<dyn std::error::Error>> {
        let x = bounds[0].as_f64().ok_or("parent longitude")?;
        let y = bounds[1].as_f64().ok_or("parent latitude")?;
        Ok((
            ((x + 180.0) / 2.0).floor() as i32,
            ((y + 90.0) / 2.0).floor() as i32,
        ))
    };
    let mut parents = BTreeMap::new();
    for cell in levels[2]["cells"]
        .as_array()
        .ok_or("missing parent cells")?
    {
        if parents.insert(parent_key(&cell["bounds"])?, cell).is_some() {
            return Err("duplicate count parent".into());
        }
    }
    let mut references = Vec::new();
    for level in &levels[4..] {
        let mut groups: BTreeMap<(i32, i32), Vec<Value>> = BTreeMap::new();
        for cell in level["cells"].as_array().ok_or("missing fine cells")? {
            groups
                .entry(parent_key(&cell["bounds"])?)
                .or_default()
                .push(cell.clone());
        }
        if groups.len() != parents.len() {
            return Err("missing fine count parent".into());
        }
        let mut chunks = Vec::new();
        let mut conserved = 0_u64;
        for (key, cells) in groups {
            let parent = parents.get(&key).ok_or("unknown count parent")?;
            let count: u64 = cells.iter().map(|c| c["count"].as_u64().unwrap()).sum();
            if count != parent["count"].as_u64().ok_or("parent count")? {
                return Err("count subtree conservation".into());
            }
            for axis in 0..2 {
                let weighted: f64 = cells
                    .iter()
                    .map(|c| {
                        c["center"][axis].as_f64().unwrap() * c["count"].as_u64().unwrap() as f64
                    })
                    .sum();
                if (weighted - parent["center"][axis].as_f64().unwrap() * count as f64).abs()
                    > 1e-7 * count as f64
                {
                    return Err("count subtree center".into());
                }
            }
            let chunk = json!({"schemaVersion":1,"kind":"gev-well-count-grids","centerMethod":"record-mean","total":count,"parentBounds":parent["bounds"],"levels":[{"degrees":level["degrees"],"total":count,"cells":cells}]});
            let mut reference = publish_count_file(out, budget, &chunk)?;
            reference["bounds"] = parent["bounds"].clone();
            reference["count"] = json!(count);
            reference["cellCount"] = json!(cells.len());
            chunks.push(reference);
            conserved += count;
        }
        if conserved != total as u64 {
            return Err("partitioned level conservation".into());
        }
        references.push(json!({"degrees":level["degrees"],"cellCount":level["cells"].as_array().unwrap().len(),"chunks":chunks}));
    }
    Ok(
        json!({"schemaVersion":2,"kind":"gev-well-partitioned-counts","total":total,"overview":overview,"levels":references}),
    )
}
fn split_detail(
    out: &Path,
    budget: &Budget,
    b: [f64; 4],
    rows: Vec<DetailRow>,
    base: &Value,
    depth: u8,
    regions: &mut Vec<Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut acqs = BTreeMap::new();
    for (_, a) in &rows {
        for v in a {
            acqs.insert(serde_json::to_string(v)?, v.clone());
        }
    }
    let mut tile = base.clone();
    tile["records"] = json!(rows.iter().map(|r| &r.0).collect::<Vec<_>>());
    tile["acquisitions"] = json!(acqs.values().collect::<Vec<_>>());
    tile["coverage"]["description"] = json!(coverage_description(&tile));
    let bytes = serde_json::to_vec(&tile)?;
    if depth < 8 && (rows.len() > 256 || bytes.len() > 1024 * 1024 || acqs.len() > 256) {
        let mx = (b[0] + b[2]) / 2.0;
        let my = (b[1] + b[3]) / 2.0;
        let mut children: [Vec<DetailRow>; 4] = std::array::from_fn(|_| Vec::new());
        for row in rows {
            let x = usize::from(row.0["longitude"].as_f64().unwrap() >= mx);
            let y = usize::from(row.0["latitude"].as_f64().unwrap() >= my);
            children[y * 2 + x].push(row);
        }
        for (i, child) in children.into_iter().enumerate() {
            if !child.is_empty() {
                let x = i % 2;
                let y = i / 2;
                split_detail(
                    out,
                    budget,
                    [
                        if x == 0 { b[0] } else { mx },
                        if y == 0 { b[1] } else { my },
                        if x == 0 { mx } else { b[2] },
                        if y == 0 { my } else { b[3] },
                    ],
                    child,
                    base,
                    depth + 1,
                    regions,
                )?;
            }
        }
    } else {
        // At maximum depth, distinct coincident source IDs remain separate.
        // Multiple chunks share bounds; the viewer uses complete counts when
        // their combined record/byte budget is exceeded.
        for chunk in rows.chunks(256) {
            let mut leaf = base.clone();
            let mut acqs = BTreeMap::new();
            for (_, a) in chunk {
                for v in a {
                    acqs.insert(serde_json::to_string(v)?, v.clone());
                }
            }
            leaf["records"] = json!(chunk.iter().map(|r| &r.0).collect::<Vec<_>>());
            leaf["acquisitions"] = json!(acqs.values().collect::<Vec<_>>());
            leaf["coverage"]["description"] = json!(coverage_description(&leaf));
            let bytes = serde_json::to_vec(&leaf)?;
            if bytes.len() > 8 * 1024 * 1024 || acqs.len() > 256 {
                return Err("detail leaf budget".into());
            }
            let digest = hash(&bytes);
            let name = format!("{digest}.json");
            write_immutable(out, budget, &name, &bytes)?;
            regions.push(json!({"url":name,"sha256":digest,"bounds":b,"count":chunk.len(),"bytes":bytes.len(),"depth":depth}));
        }
    }
    Ok(())
}
// Historical rows have already passed global source-ID reconciliation. Split
// dense groups by streaming existing bounded tiles before collecting detail rows.
// This avoids a dense half-degree cell becoming an unbounded JSON tree in RAM.
fn flush_historical_child(
    out: &Path,
    budget: &Budget,
    base: &Value,
    bounds: [f64; 4],
    rows: &mut Vec<Value>,
    children: &mut Vec<Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    if rows.is_empty() {
        return Ok(());
    }
    let count = rows.len();
    if count > 500 {
        return Err("historical stage row cap".into());
    }
    let mut pack = base.clone();
    pack["records"] = json!(std::mem::take(rows));
    let bytes = serde_json::to_vec(&pack)?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("historical stage byte cap".into());
    }
    let digest = hash(&bytes);
    let name = format!("{digest}.json");
    write_immutable(out, budget, &name, &bytes)?;
    children.push(
        json!({"url":name,"sha256":digest,"bounds":bounds,"count":count,"bytes":bytes.len()}),
    );
    Ok(())
}
fn compact_historical_group(
    out: &Path,
    budget: &Budget,
    tiles: Vec<Value>,
    b: [f64; 4],
    depth: u8,
    regions: &mut Vec<Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    let total = tiles.iter().try_fold(0u64, |n, t| {
        n.checked_add(t["count"].as_u64().ok_or("historical stage count")?)
            .ok_or("historical stage count overflow")
    })?;
    if total <= 2000 {
        let mut rows = BTreeMap::new();
        let mut base = Value::Null;
        for tile in &tiles {
            let pack = read_detail_stage(out, budget, tile)?;
            if base.is_null() {
                base = pack.clone();
                base["records"] = json!([]);
            }
            if pack["acquisitions"] != base["acquisitions"] {
                return Err("historical stage acquisition mismatch".into());
            }
            let acqs: Vec<Value> = serde_json::from_value(pack["acquisitions"].clone())?;
            for r in pack["records"].as_array().unwrap() {
                if rows
                    .insert(
                        r["id"]
                            .as_str()
                            .ok_or("historical stage identity")?
                            .to_owned(),
                        (r.clone(), acqs.clone()),
                    )
                    .is_some()
                {
                    return Err("duplicate historical stage ID".into());
                }
            }
        }
        if rows.len() as u64 != total {
            return Err("historical stage conservation".into());
        }
        return split_detail(
            out,
            budget,
            b,
            rows.into_values().collect(),
            &base,
            depth,
            regions,
        );
    }
    let mx = (b[0] + b[2]) / 2.0;
    let my = (b[1] + b[3]) / 2.0;
    let mut children: [Vec<Value>; 4] = std::array::from_fn(|_| Vec::new());
    let mut buffers: [Vec<Value>; 4] = std::array::from_fn(|_| Vec::new());
    let mut buffered_bytes = [0usize; 4];
    let mut base = Value::Null;
    let mut row_byte_limit = 0usize;
    let child_bounds: [[f64; 4]; 4] = std::array::from_fn(|i| {
        [
            if i % 2 == 0 { b[0] } else { mx },
            if i / 2 == 0 { b[1] } else { my },
            if i % 2 == 0 { mx } else { b[2] },
            if i / 2 == 0 { my } else { b[3] },
        ]
    });
    let mut observed = 0u64;
    for tile in &tiles {
        let mut pack = read_detail_stage(out, budget, tile)?;
        let records = pack["records"].as_array_mut().unwrap();
        let rows = std::mem::take(records);
        observed += rows.len() as u64;
        if depth == 8 {
            let acqs: Vec<Value> = serde_json::from_value(pack["acquisitions"].clone())?;
            split_detail(
                out,
                budget,
                b,
                rows.into_iter().map(|r| (r, acqs.clone())).collect(),
                &pack,
                depth,
                regions,
            )?;
            continue;
        }
        if base.is_null() {
            base = pack.clone();
            let overhead = serde_json::to_vec(&base)?.len();
            row_byte_limit = (8 * 1024 * 1024usize)
                .checked_sub(overhead)
                .ok_or("historical stage header cap")?
                .min(4 * 1024 * 1024);
        } else if pack != base {
            return Err("historical stage header mismatch".into());
        }
        for row in rows {
            let x = usize::from(
                row["longitude"]
                    .as_f64()
                    .ok_or("historical stage longitude")?
                    >= mx,
            );
            let y = usize::from(
                row["latitude"]
                    .as_f64()
                    .ok_or("historical stage latitude")?
                    >= my,
            );
            let i = y * 2 + x;
            let bytes = serde_json::to_vec(&row)?.len() + 1;
            if !buffers[i].is_empty()
                && (buffers[i].len() == 500 || buffered_bytes[i] + bytes > row_byte_limit)
            {
                flush_historical_child(
                    out,
                    budget,
                    &base,
                    child_bounds[i],
                    &mut buffers[i],
                    &mut children[i],
                )?;
                buffered_bytes[i] = 0;
            }
            buffered_bytes[i] += bytes;
            buffers[i].push(row);
            if buffers[i].len() == 500 || buffered_bytes[i] >= row_byte_limit {
                flush_historical_child(
                    out,
                    budget,
                    &base,
                    child_bounds[i],
                    &mut buffers[i],
                    &mut children[i],
                )?;
                buffered_bytes[i] = 0;
            }
        }
    }
    if observed != total {
        return Err("historical stage input conservation".into());
    }
    for i in 0..4 {
        flush_historical_child(
            out,
            budget,
            &base,
            child_bounds[i],
            &mut buffers[i],
            &mut children[i],
        )?;
    }
    for child in children {
        if child.is_empty() {
            continue;
        }
        let bounds = serde_json::from_value(child[0]["bounds"].clone())?;
        compact_historical_group(out, budget, child, bounds, depth + 1, regions)?;
    }
    if regions.len() > 40000 {
        return Err("detail manifest entry cap".into());
    }
    Ok(())
}
fn read_detail_stage(
    out: &Path,
    budget: &Budget,
    tile: &Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    budget.check()?;
    let digest = tile["sha256"].as_str().ok_or("historical stage hash")?;
    if digest.len() != 64
        || !digest.bytes().all(|b| b.is_ascii_hexdigit())
        || tile["url"] != format!("{digest}.json")
    {
        return Err("historical stage path".into());
    }
    let path = out.join(tile["url"].as_str().unwrap());
    if fs::metadata(&path)?.len() > 8 * 1024 * 1024 {
        return Err("historical stage file cap".into());
    }
    let bytes = fs::read(path)?;
    if hash(&bytes) != digest || tile["bytes"] != bytes.len() {
        return Err("historical stage integrity".into());
    }
    let pack: Value = serde_json::from_slice(&bytes)?;
    let rows = pack["records"].as_array().ok_or("historical stage rows")?;
    if rows.len() > 500 || tile["count"] != rows.len() {
        return Err("historical stage row cap".into());
    }
    Ok(pack)
}
fn compact_details(
    out: &Path,
    budget: &Budget,
    old: Vec<Value>,
    historical: bool,
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let mut groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for tile in old {
        groups
            .entry(serde_json::to_string(&tile["bounds"])?)
            .or_default()
            .push(tile);
    }
    let mut regions = Vec::new();
    for tiles in groups.values() {
        let b: [f64; 4] = serde_json::from_value(tiles[0]["bounds"].clone())?;
        if historical
            && tiles
                .iter()
                .map(|t| t["count"].as_u64().unwrap_or(0))
                .sum::<u64>()
                > 2000
        {
            compact_historical_group(out, budget, tiles.clone(), b, 0, &mut regions)?;
            continue;
        }
        let mut rows = BTreeMap::new();
        let mut base = Value::Null;
        for tile in tiles {
            budget.check()?;
            let bytes = fs::read(out.join(tile["url"].as_str().unwrap()))?;
            if hash(&bytes) != tile["sha256"].as_str().unwrap() {
                return Err("detail input changed".into());
            }
            let pack: Value = serde_json::from_slice(&bytes)?;
            if base.is_null() {
                base = pack.clone();
                base["records"] = json!([]);
            }
            let acqs: Vec<Value> = serde_json::from_value(pack["acquisitions"].clone())?;
            for record in pack["records"].as_array().unwrap() {
                if rows
                    .insert(
                        record["id"].as_str().unwrap().to_owned(),
                        (record.clone(), acqs.clone()),
                    )
                    .is_some()
                {
                    return Err("duplicate detail source ID".into());
                }
            }
        }
        split_detail(
            out,
            budget,
            b,
            rows.into_values().collect(),
            &base,
            0,
            &mut regions,
        )?;
        if regions.len() > 40000 {
            return Err("detail manifest entry cap".into());
        }
    }
    Ok(regions)
}
// Historical acquisitions are constant for a state. Buffer a bounded number of
// already admitted rows per existing half-degree cell, avoiding a durable tiny
// file for each source-partition/cell intersection. Final compaction is unchanged.
#[derive(Default)]
struct HistoricalCellBuffer {
    cells: BTreeMap<(i32, i32), (Vec<Value>, usize)>,
    rows: usize,
    bytes: usize,
}
impl HistoricalCellBuffer {
    fn flush(
        &mut self,
        key: (i32, i32),
        out: &Path,
        budget: &Budget,
        base: &Value,
        regions: &mut Vec<Value>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (mut rows, bytes) = self
            .cells
            .remove(&key)
            .ok_or("historical cell buffer key")?;
        self.rows -= rows.len();
        self.bytes -= bytes;
        rows.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        write_initial_cell(out, budget, key, &rows, base, regions)
    }
    fn add(
        &mut self,
        key: (i32, i32),
        row: Value,
        out: &Path,
        budget: &Budget,
        base: &Value,
        regions: &mut Vec<Value>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        budget.check()?;
        let bytes = serde_json::to_vec(&row)?.len();
        let cell = self.cells.entry(key).or_default();
        cell.0.push(row);
        cell.1 += bytes;
        self.rows += 1;
        self.bytes += bytes;
        if cell.0.len() >= 500 || cell.1 >= 4 * 1024 * 1024 {
            self.flush(key, out, budget, base, regions)?;
        }
        while self.rows > 10_000 || self.bytes > 64 * 1024 * 1024 {
            let key = *self
                .cells
                .iter()
                .max_by_key(|(key, (_, bytes))| (*bytes, **key))
                .ok_or("historical buffer accounting")?
                .0;
            self.flush(key, out, budget, base, regions)?;
        }
        Ok(())
    }
}
fn write_initial_cell(
    out: &Path,
    budget: &Budget,
    key: (i32, i32),
    rows: &[Value],
    base: &Value,
    regions: &mut Vec<Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    for chunk in rows.chunks(500) {
        let mut tile = base.clone();
        tile["records"] = json!(chunk);
        tile["coverage"]["indexed"] = json!(true);
        tile["coverage"]["description"] = json!(coverage_description(&tile));
        let bytes = serde_json::to_vec(&tile)?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err("tile byte cap".into());
        }
        let digest = hash(&bytes);
        let name = format!("{digest}.json");
        write_immutable(out, budget, &name, &bytes)?;
        let (x, y) = key;
        regions.push(json!({"url":name,"sha256":digest,"count":chunk.len(),"bytes":bytes.len(),"bounds":[x as f64/2.0,y as f64/2.0,(x+1) as f64/2.0,(y+1) as f64/2.0]}));
    }
    Ok(())
}

fn run(input: &Path, out: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    run_input(input, out, false)
}
fn run_input(
    input: &Path,
    out: &Path,
    historical: bool,
) -> Result<Value, Box<dyn std::error::Error>> {
    run_input_with_seconds(input, out, historical, 300)
}
fn run_input_with_seconds(
    input: &Path,
    out: &Path,
    historical: bool,
    seconds: u64,
) -> Result<Value, Box<dyn std::error::Error>> {
    if !(1..=900).contains(&seconds) || (!historical && seconds != 300) {
        return Err("historical deadline must be 1..900 seconds".into());
    }
    let mut budget_owner = Budget::new();
    budget_owner.seconds = seconds;
    let budget = &budget_owner;
    let started = budget.started;
    let mut historical_input = if historical {
        Some(gev_us_groundwater::usgwd_map::Input::open_with_seconds(
            input, started, seconds,
        )?)
    } else {
        None
    };
    let descriptor: Value = match &historical_input {
        Some(h) => {
            json!({"snapshot":{"source":"USGWD","releaseSha256":h.source["releaseSha256"],"normalizationManifestSha256":h.manifest_sha256,"catalogAccounting":h.accounting,"meaning":"Historical source records; physical-well identity unresolved; no proximity merge"},"candidates":h.candidates})
        }
        None => serde_json::from_slice(&fs::read(input)?)?,
    };
    let candidates = descriptor["candidates"]
        .as_array()
        .ok_or("missing candidates")?;
    if candidates.len() > if historical { 10_016 } else { 2000 } {
        return Err("candidate limit".into());
    }
    let _writer = IndexWriterLock::acquire(out)?;
    let mut regions = Vec::new();
    let sizes = [32.0_f64, 8.0, 2.0, 0.5, 0.125, 0.03125];
    let mut grids: Vec<BTreeMap<(i32, i32), (u64, f64, f64)>> =
        sizes.iter().map(|_| BTreeMap::new()).collect();
    let mut seen: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut overlap_batches = Vec::new();
    let mut overlap_count = 0usize;
    let mut differing_versions = 0usize;
    let mut historical_cells = HistoricalCellBuffer::default();
    let mut source_rows = 0usize;
    let mut anonymous_quarantines = 0u64;
    let mut measurement_flagged = std::collections::BTreeSet::new();
    let mut fallback = historical_input.as_ref().map_or(Value::Null, |h| gev_us_groundwater::usgwd_map::pack(Vec::new(), &h.source, &h.manifest_sha256)["acquisitions"][0].clone());
    for (candidate_number, item) in candidates.iter().enumerate() {
        if candidate_number % 100 == 0 {
            eprintln!(
                "well_index: validating candidate {}/{}",
                candidate_number + 1,
                candidates.len()
            );
        }
        budget.check()?;
        let pack_value;
        let candidate;
        let pack = if let Some(h) = &mut historical_input {
            let part = h.partition(item)?;
            let rows = part["records"]
                .as_array()
                .ok_or("historical partition records")?;
            let quarantine = part["quarantine"]
                .as_array()
                .ok_or("historical partition quarantine")?;
            if rows.len() + quarantine.len() > 200 {
                return Err("historical partition row cap".into());
            }
            let count = rows.len() + quarantine.len();
            if part["firstRow"] != source_rows + 2 || part["lastRow"] != source_rows + count + 1 {
                return Err("historical source row interval".into());
            }
            let mut ordinals: Vec<u64> = rows
                .iter()
                .map(|r| r["provenance"]["sourceRow"].as_u64().unwrap_or(0))
                .chain(
                    quarantine
                        .iter()
                        .map(|q| q["sourceRow"].as_u64().unwrap_or(0)),
                )
                .collect();
            ordinals.sort();
            if ordinals
                != ((source_rows + 2) as u64..=(source_rows + count + 1) as u64).collect::<Vec<_>>()
            {
                return Err("historical source row ordinal conservation".into());
            }
            for row in quarantine {
                if let Some(id) = row["sourceId"].as_str() {
                    let n = h
                        .quarantined
                        .get_mut(id)
                        .ok_or("historical quarantine index membership")?;
                    *n = n
                        .checked_sub(1)
                        .ok_or("historical quarantine occurrence excess")?;
                } else if row["sourceId"].is_null() {
                    anonymous_quarantines += 1;
                } else {
                    return Err("historical quarantine identity".into());
                }
            }
            source_rows += count;
            let mut records = Vec::new();
            for row in rows {
                let mut content = row.clone();
                let object = content.as_object_mut().ok_or("historical record object")?;
                object.remove("provenance");
                object.remove("contentSha256");
                if row["contentSha256"] != hash(&serde_json::to_vec(&content)?) {
                    return Err("historical record content hash mismatch".into());
                }
                if row["provenance"]["inputSha256"] != h.source["inputSha256"] {
                    return Err("historical record input provenance mismatch".into());
                }
                let id = row["id"].as_str().ok_or("historical record id")?;
                if let Some((digest, remaining, mapped)) = h.accepted.get_mut(id) {
                    *remaining = remaining
                        .checked_sub(1)
                        .ok_or("historical accepted occurrence excess")?;
                    if row["mapEligible"] != *mapped {
                        return Err("historical accepted map eligibility mismatch".into());
                    }
                    if row["quantityErrors"]
                        .as_array()
                        .is_some_and(|e| !e.is_empty())
                    {
                        measurement_flagged.insert(id.to_owned());
                    }
                    if row["contentSha256"] != digest.as_str() {
                        return Err("historical authoritative index mismatch".into());
                    }
                    if let Some(record) = gev_us_groundwater::usgwd_map::record(row, &h.source)? {
                        records.push(record);
                    }
                } else {
                    let remaining = h
                        .quarantined
                        .get_mut(id)
                        .ok_or("historical record index membership")?;
                    *remaining = remaining
                        .checked_sub(1)
                        .ok_or("historical poisoned occurrence excess")?;
                }
            }
            pack_value =
                gev_us_groundwater::usgwd_map::pack(records, &h.source, &h.manifest_sha256);
            &pack_value
        } else {
            let path = input
                .parent()
                .unwrap()
                .join(item["path"].as_str().ok_or("missing path")?);
            if fs::metadata(&path)?.len() > 8 * 1024 * 1024 {
                return Err("candidate byte cap".into());
            }
            let raw = fs::read(path)?;
            if Some(hash(&raw).as_str()) != item["sha256"].as_str() {
                return Err("candidate hash mismatch".into());
            }
            candidate = serde_json::from_slice::<Value>(&raw)?;
            gev_us_groundwater::validate_bundle(&candidate)?;
            &candidate["layerPack"]
        };
        if fallback.is_null() {
            fallback = pack["acquisitions"][0].clone();
        }
        let mut cells: BTreeMap<(i32, i32), Vec<Value>> = BTreeMap::new();
        let mut duplicates = Vec::new();
        for r in pack["records"].as_array().ok_or("missing records")? {
            budget.check()?;
            let id = r["id"].as_str().ok_or("missing id")?;
            let record_hash = hash(&serde_json::to_vec(r)?);
            if let Some((retained_hash, retained_candidate)) = seen.get(id) {
                if !historical && item["overlapPolicy"] != "retain-first-with-audit" {
                    return Err(format!("duplicate source ID: {id}").into());
                }
                overlap_count += 1;
                if retained_hash != &record_hash {
                    differing_versions += 1;
                }
                duplicates.push(json!({"id":id,"retainedRecordSha256":retained_hash,"retainedCandidateSha256":retained_candidate,"alternateRecordSha256":record_hash}));
                continue;
            }
            seen.insert(
                id.to_owned(),
                (record_hash, item["sha256"].as_str().unwrap().to_owned()),
            );
            let lon = r["longitude"]
                .as_f64()
                .filter(|x| x.is_finite() && (-180.0..=180.0).contains(x))
                .ok_or("longitude")?;
            let lat = r["latitude"]
                .as_f64()
                .filter(|x| x.is_finite() && (-90.0..=90.0).contains(x))
                .ok_or("latitude")?;
            for (grid, size) in grids.iter_mut().zip(sizes) {
                let x = ((lon.min(179.999999999) + 180.0) / size).floor() as i32;
                let y = ((lat.min(89.999999999) + 90.0) / size).floor() as i32;
                let value = grid.entry((x, y)).or_default();
                value.0 += 1;
                value.1 += lon;
                value.2 += lat;
            }
            cells
                .entry(((lon * 2.0).floor() as i32, (lat * 2.0).floor() as i32))
                .or_default()
                .push(r.clone());
        }
        if !duplicates.is_empty() {
            overlap_batches.push(json!({"candidateSha256":item["sha256"],"acquisitions":pack["acquisitions"],"duplicates":duplicates}));
        }
        for (key, mut rows) in cells {
            if historical {
                for row in rows {
                    historical_cells.add(key, row, out, budget, pack, &mut regions)?;
                }
            } else {
                rows.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
                write_initial_cell(out, budget, key, &rows, pack, &mut regions)?;
            }
        }
    }
    if let Some(h) = &historical_input {
        if h.accounting["inputRows"] != source_rows
            || h.accepted.values().any(|(_, remaining, _)| *remaining != 0)
            || h.quarantined.values().any(|remaining| *remaining != 0)
            || anonymous_quarantines != h.anonymous_quarantines
        {
            return Err("historical partition row conservation".into());
        }
    }
    if fallback.is_null() {
        return Err("empty source descriptor".into());
    }
    if let Some(expected) = descriptor["expectedUnique"].as_u64() {
        if seen.len() as u64 != expected {
            return Err(format!(
                "computed unique count {} differs from expected {expected}",
                seen.len()
            )
            .into());
        }
    }
    if let Some(h) = &historical_input {
        let base = gev_us_groundwater::usgwd_map::pack(Vec::new(), &h.source, &h.manifest_sha256);
        while let Some(key) = historical_cells.cells.keys().next().copied() {
            historical_cells.flush(key, out, budget, &base, &mut regions)?;
        }
        if historical_cells.rows != 0 || historical_cells.bytes != 0 {
            return Err("historical buffer drain accounting".into());
        }
    }
    regions = compact_details(out, budget, regions, historical)?;
    if regions
        .iter()
        .map(|r| r["count"].as_u64().unwrap())
        .sum::<u64>()
        != seen.len() as u64
    {
        return Err("detail conservation".into());
    }
    let audit = json!({"schemaVersion":1,"policy":"retain-first-with-audit","overlapIds":overlap_count,"differingSerializedVersions":differing_versions,"meaning":"Serialized differences include retrieval/projection differences; not a claim of changed measurements","batches":overlap_batches});
    let audit_bytes = serde_json::to_vec(&audit)?;
    let audit_hash = hash(&audit_bytes);
    let audit_name = format!("{audit_hash}.audit.json");
    write_immutable(out, budget, &audit_name, &audit_bytes)?;
    regions.sort_by_key(|v| v["url"].as_str().unwrap().to_owned());
    let aggregate_levels: Vec<Value> = grids.iter().zip(sizes).map(|(grid,size)| {
        let total: u64 = grid.values().map(|v|v.0).sum();
        assert_eq!(total, seen.len() as u64);
        let cells: Vec<Value> = grid.iter().map(|(&(x,y),&(count,lon_sum,lat_sum))| json!({"center":[lon_sum/count as f64,lat_sum/count as f64],"bounds":[x as f64*size-180.0,y as f64*size-90.0,((x+1) as f64*size-180.0).min(180.0),((y+1) as f64*size-90.0).min(90.0)],"count":count})).collect();
        json!({"degrees":size,"total":total,"cells":cells})
    }).collect();
    let aggregate_reference =
        publish_partitioned_counts(out, budget, &aggregate_levels, seen.len())?;
    let mut index = json!({"schemaVersion":1,"kind":"gev-well-spatial-index","coverageComplete":false,"snapshot":descriptor["snapshot"],"indexedRecords":seen.len(),"overlapAudit":{"url":audit_name,"sha256":audit_hash,"overlapIds":overlap_count,"differingSerializedVersions":differing_versions},"fallbackAcquisition":fallback,"regions":regions,"aggregates":aggregate_reference});
    if let Some(h) = &historical_input {
        index["sourceKind"] = json!("USGWD");
        index["normalizationPolicy"] = json!(gev_us_groundwater::usgwd_map::POLICY);
        index["catalogAccounting"] = h.accounting.clone();
        index["catalogRecords"] = h.accounting["acceptedRecords"].clone();
        index["unmappedRecords"] =
            json!(h.accounting["acceptedRecords"].as_u64().unwrap() - seen.len() as u64);
        index["measurementFlaggedCatalogRecords"] = json!(measurement_flagged.len());
        index["mappingExclusions"] = json!({"sourceCoordinatesOrQuality":h.accounting["nonmappedRecords"],"unsupportedDatumOperationArea":h.accounting["mapEligibleRecords"].as_u64().unwrap().checked_sub(seen.len() as u64).ok_or("historical map eligibility conservation")?});
        index["distinctPhysicalWells"] = Value::Null;
        index["sourceState"] = h.source["state"].clone();
        index["identityRange"] = json!({"first":h.accepted.keys().next(),"last":h.accepted.keys().next_back(),"releaseSha256":h.source["releaseSha256"],"meaning":"release-scoped catalog IDs, not physical wells"});
    }
    let bytes = serde_json::to_vec(&index)?;
    budget.charge(bytes.len())?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("index byte cap".into());
    }
    if out.join("index.json").exists() && fs::read(out.join("index.json"))? == bytes {
        return Ok(
            json!({"indexedRecords":seen.len(),"packs":regions.len(),"unchanged":true,"indexSha256":hash(&bytes)}),
        );
    }
    // Every referenced tile is durable before replacing the only mutable pointer.
    // Interrupted/invalid runs leave the previous index usable; orphan tiles are harmless.
    if out.join("index.json").exists() {
        let previous = fs::read(out.join("index.json"))?;
        write_immutable(
            out,
            budget,
            &format!("{}.index.json", hash(&previous)),
            &previous,
        )?;
    }
    let pending = out.join("index.pending");
    fs::write(&pending, &bytes)?;
    fs::OpenOptions::new()
        .write(true)
        .open(&pending)?
        .sync_all()
        .map_err(|e| format!("index flush: {e}"))?;
    fs::rename(&pending, out.join("index.json")).map_err(|e| format!("index replacement: {e}"))?;
    Ok(json!({"indexedRecords":seen.len(),"packs":regions.len(),"indexSha256":hash(&bytes)}))
}
#[path = "../well_index_merge.rs"]
mod federation;

#[cfg(test)]
mod tests {
    use super::*;
    fn scratch(label: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "well-index-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }
    fn descriptor_for(dir: &Path, inventory: bool) -> std::path::PathBuf {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let candidate = if inventory {
            gev_us_groundwater::inventory_bundle(
                &fs::read(root.join("fixtures/us-groundwater/usgs-inventory-pilot/capture.json"))
                    .unwrap(),
                "36",
            )
            .unwrap()
        } else {
            gev_us_groundwater::bundle(
                &fs::read(root.join("fixtures/us-groundwater/v1/capture.json")).unwrap(),
            )
            .unwrap()
        };
        let bytes = serde_json::to_vec(&candidate).unwrap();
        fs::write(dir.join("candidate.json"), &bytes).unwrap();
        let descriptor = dir.join("batches.json");
        fs::write(&descriptor, serde_json::to_vec(&json!({"snapshot":"offline-test","candidates":[{"path":"candidate.json","sha256":hash(&bytes)}]})).unwrap()).unwrap();
        descriptor
    }
    #[test]
    fn historical_buffer_evicts_without_losing_accounted_source_ids() {
        let dir = scratch("historical-buffer");
        let budget = Budget::new();
        let mut buffer = HistoricalCellBuffer::default();
        let mut regions = Vec::new();
        let base = json!({"coverage":{"complete":false},"acquisitions":[],"records":[]});
        for i in 0..10_001 {
            buffer
                .add(
                    (i % 720 - 360, i / 720 - 180),
                    json!({"id":format!("SYN_{i}")}),
                    &dir,
                    &budget,
                    &base,
                    &mut regions,
                )
                .unwrap();
            assert!(buffer.rows <= 10_000 && buffer.bytes <= 64 * 1024 * 1024);
        }
        assert_eq!(
            buffer.rows
                + regions
                    .iter()
                    .map(|r| r["count"].as_u64().unwrap() as usize)
                    .sum::<usize>(),
            10_001
        );
        assert_eq!(regions.len(), 1);
    }
    #[test]
    fn historical_stage_refuses_tampering_oversized_rows_and_expired_budget() {
        let dir = scratch("historical-stage");
        let budget = Budget::new();
        let pack = json!({"records":vec![json!({"id":"x"});501],"acquisitions":[]});
        let bytes = serde_json::to_vec(&pack).unwrap();
        let digest = hash(&bytes);
        let name = format!("{digest}.json");
        write_immutable(&dir, &budget, &name, &bytes).unwrap();
        let reference = json!({"url":name,"sha256":digest,"bytes":bytes.len(),"count":501});
        assert!(read_detail_stage(&dir, &budget, &reference).is_err());
        let mut wrong = reference.clone();
        wrong["url"] = json!("../outside.json");
        assert!(read_detail_stage(&dir, &budget, &wrong).is_err());
        let expired = Budget {
            started: std::time::Instant::now() - std::time::Duration::from_secs(301),
            bytes: std::cell::Cell::new(0),
            seconds: 300,
        };
        assert!(read_detail_stage(&dir, &expired, &reference).is_err());
        fs::write(dir.join(name), b"{}").unwrap();
        assert!(read_detail_stage(&dir, &budget, &reference).is_err());
    }
    #[test]
    fn output_and_deadline_guards_refuse_before_immutable_write() {
        let dir = scratch("budget");
        let budget = Budget::new();
        budget.bytes.set(16 * 1024 * 1024 * 1024);
        assert!(write_immutable(&dir, &budget, "refused.json", b"x").is_err());
        assert!(!dir.join("refused.json").exists());
        let expired = Budget {
            started: std::time::Instant::now() - std::time::Duration::from_secs(301),
            bytes: std::cell::Cell::new(0),
            seconds: 300,
        };
        assert!(write_immutable(&dir, &expired, "expired.json", b"x").is_err());
        assert!(!dir.join("expired.json").exists());
    }
    #[test]
    fn historical_subdivision_coalesces_fragmented_stages_without_losing_ids() {
        let dir = scratch("historical-fragmented");
        let budget = Budget::new();
        let mut old = Vec::new();
        for batch in 0..210 {
            let rows:Vec<_>=(0..10).map(|n|{let i=batch*10+n;json!({"id":format!("ROW-{i:05}"),"longitude":if i%2==0{0.1}else{0.4},"latitude":if i%4<2{0.1}else{0.4}})}).collect();
            let pack = json!({"coverage":{"indexed":true},"acquisitions":[],"records":rows});
            let bytes = serde_json::to_vec(&pack).unwrap();
            let digest = hash(&bytes);
            let name = format!("{digest}.json");
            write_immutable(&dir, &budget, &name, &bytes).unwrap();
            old.push(json!({"url":name,"sha256":digest,"bytes":bytes.len(),"count":10,"bounds":[0.0,0.0,0.5,0.5]}));
        }
        let pack = json!({"coverage":{"indexed":true},"acquisitions":[],"records":[{"id":"LARGE","longitude":0.1,"latitude":0.1,"payload":"x".repeat(5*1024*1024)}]});
        let bytes = serde_json::to_vec(&pack).unwrap();
        let digest = hash(&bytes);
        let name = format!("{digest}.json");
        write_immutable(&dir, &budget, &name, &bytes).unwrap();
        old.push(json!({"url":name,"sha256":digest,"bytes":bytes.len(),"count":1,"bounds":[0.0,0.0,0.5,0.5]}));
        let before = fs::read_dir(&dir).unwrap().count();
        let result = compact_details(&dir, &budget, old.clone(), true).unwrap();
        assert!(
            fs::read_dir(&dir).unwrap().count() - before <= 40,
            "fragmented input must not create one child artifact per input tile"
        );
        let mut ids = std::collections::BTreeSet::new();
        for r in &result {
            let bytes = fs::read(dir.join(r["url"].as_str().unwrap())).unwrap();
            assert!(bytes.len() <= 8 * 1024 * 1024);
            let p: Value = serde_json::from_slice(&bytes).unwrap();
            for row in p["records"].as_array().unwrap() {
                assert!(ids.insert(row["id"].as_str().unwrap().to_owned()));
            }
        }
        assert_eq!(ids.len(), 2101);
        assert!(ids.contains("LARGE"));
        assert_eq!(result, compact_details(&dir, &budget, old, true).unwrap());
    }
    #[test]
    fn explicit_historical_deadline_is_finite_and_applies_to_final_writes() {
        let dir = scratch("explicit-deadline");
        for seconds in [0, 901, u64::MAX] {
            assert!(run_input_with_seconds(Path::new("missing"), &dir, true, seconds).is_err());
        }
        let mut budget = Budget::new();
        budget.seconds = 900;
        budget.started = std::time::Instant::now() - std::time::Duration::from_secs(301);
        assert!(budget.check().is_ok());
        budget.started = std::time::Instant::now() - std::time::Duration::from_secs(901);
        assert!(write_immutable(&dir, &budget, "expired.json", b"x").is_err());
        assert!(!dir.join("expired.json").exists());
    }
    #[test]
    fn partitioned_levels_conserve_all_cells_and_reuse_unchanged_subtrees() {
        let dir = scratch("partitioned");
        let make_levels = |points: &[(f64, f64)]| -> Vec<Value> {
            [32.0_f64,8.0,2.0,0.5,0.125,0.03125].iter().map(|size| {
                let mut groups: BTreeMap<(i32,i32),(u64,f64,f64)> = BTreeMap::new();
                for &(lon,lat) in points {
                    let entry = groups.entry((((lon+180.0)/size).floor() as i32,((lat+90.0)/size).floor() as i32)).or_default();
                    entry.0+=1;entry.1+=lon;entry.2+=lat;
                }
                let cells:Vec<Value>=groups.iter().map(|(&(x,y),&(count,lon,lat))|json!({"bounds":[x as f64*size-180.0,y as f64*size-90.0,((x+1) as f64*size-180.0).min(180.0),((y+1) as f64*size-90.0).min(90.0)],"count":count,"center":[lon/count as f64,lat/count as f64]})).collect();
                json!({"degrees":size,"total":points.len(),"cells":cells})
            }).collect()
        };
        let points = [
            (-0.01, 1.0),
            (0.0, 1.0),
            (0.01, 1.0),
            (2.01, 1.0),
            (-179.99, 1.0),
            (179.99, 1.0),
        ];
        let levels = make_levels(&points);
        let reference =
            publish_partitioned_counts(&dir, &Budget::new(), &levels, points.len()).unwrap();
        assert_eq!(reference["schemaVersion"], 2);
        let overview: Value = serde_json::from_slice(
            &fs::read(dir.join(reference["overview"]["url"].as_str().unwrap())).unwrap(),
        )
        .unwrap();
        assert_eq!(overview["levels"].as_array().unwrap(), &levels[..4]);
        for (i, level) in reference["levels"].as_array().unwrap().iter().enumerate() {
            let mut actual = BTreeMap::new();
            let mut sum = 0;
            for part in level["chunks"].as_array().unwrap() {
                let bytes = fs::read(dir.join(part["url"].as_str().unwrap())).unwrap();
                assert_eq!(hash(&bytes), part["sha256"].as_str().unwrap());
                assert_eq!(bytes.len() as u64, part["bytes"].as_u64().unwrap());
                let chunk: Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(chunk["parentBounds"], part["bounds"]);
                assert_eq!(chunk["total"], part["count"]);
                sum += part["count"].as_u64().unwrap();
                for cell in chunk["levels"][0]["cells"].as_array().unwrap() {
                    assert!(
                        actual
                            .insert(
                                serde_json::to_string(&cell["bounds"]).unwrap(),
                                cell.clone()
                            )
                            .is_none()
                    );
                }
            }
            let expected: BTreeMap<_, _> = levels[i + 4]["cells"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| (serde_json::to_string(&c["bounds"]).unwrap(), c.clone()))
                .collect();
            assert_eq!(actual, expected);
            assert_eq!(sum, points.len() as u64);
        }
        let first = reference["levels"][0]["chunks"][0].clone();
        let stamp = fs::metadata(dir.join(first["url"].as_str().unwrap()))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(
            publish_partitioned_counts(&dir, &Budget::new(), &levels, points.len()).unwrap(),
            reference
        );
        let mut extra = points.to_vec();
        extra.push((100.0, 30.0));
        let updated =
            publish_partitioned_counts(&dir, &Budget::new(), &make_levels(&extra), extra.len())
                .unwrap();
        assert!(
            updated["levels"][0]["chunks"]
                .as_array()
                .unwrap()
                .contains(&first)
        );
        assert_eq!(
            stamp,
            fs::metadata(dir.join(first["url"].as_str().unwrap()))
                .unwrap()
                .modified()
                .unwrap()
        );
        let mut bad = levels;
        bad[4]["cells"][0]["count"] = json!(999);
        fs::write(dir.join("index.json"), b"last good pointer").unwrap();
        assert!(publish_partitioned_counts(&dir, &Budget::new(), &bad, points.len()).is_err());
        assert_eq!(
            fs::read(dir.join("index.json")).unwrap(),
            b"last good pointer"
        );
    }
    #[test]
    fn observation_coverage_follows_actual_acquisitions_and_readings() {
        for inventory in [false, true] {
            let dir = scratch(if inventory {
                "inventory"
            } else {
                "observations"
            });
            let descriptor = descriptor_for(&dir, inventory);
            let out = dir.join("out");
            run(&descriptor, &out).unwrap();
            let index: Value =
                serde_json::from_slice(&fs::read(out.join("index.json")).unwrap()).unwrap();
            let mut readings = 0;
            for tile in index["regions"].as_array().unwrap() {
                let pack: Value = serde_json::from_slice(
                    &fs::read(out.join(tile["url"].as_str().unwrap())).unwrap(),
                )
                .unwrap();
                readings += pack["records"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| r["readings"].as_array().unwrap().len())
                    .sum::<usize>();
                let description = pack["coverage"]["description"].as_str().unwrap();
                assert!(!description.contains("No observations requested"));
                assert!(description.contains(if inventory {
                    "Inventory metadata only"
                } else {
                    "Historical observations are included"
                }));
            }
            assert_eq!(readings, if inventory { 0 } else { 6 });
        }
        let queried = json!({"records":[],"acquisitions":[{"collection":"field-measurements"}]});
        assert!(coverage_description(&queried).contains("queried; this pack contains no readings"));
        assert!(coverage_description(&json!({})).contains("scope is unknown"));
    }
    #[test]
    fn competing_writer_refused_and_interrupted_staging_retries_without_pointer_loss() {
        let dir = scratch("writer");
        let descriptor = descriptor_for(&dir, false);
        let out = dir.join("out");
        run(&descriptor, &out).unwrap();
        let last_good = fs::read(out.join("index.json")).unwrap();
        let lock = IndexWriterLock::acquire(&out).unwrap();
        fs::write(out.join("index.pending"), b"interrupted pointer staging").unwrap();
        assert!(
            run(&descriptor, &out)
                .unwrap_err()
                .to_string()
                .contains("writer lock")
        );
        assert_eq!(
            fs::read(out.join("index.pending")).unwrap(),
            b"interrupted pointer staging"
        );
        assert_eq!(fs::read(out.join("index.json")).unwrap(), last_good);
        drop(lock);
        let mut next: Value = serde_json::from_slice(&fs::read(&descriptor).unwrap()).unwrap();
        next["snapshot"] = json!("next-explicit-snapshot");
        fs::write(&descriptor, serde_json::to_vec(&next).unwrap()).unwrap();
        run(&descriptor, &out).unwrap();
        assert!(!out.join(".well-index.lock").exists());
        assert!(!out.join("index.pending").exists());
        assert_eq!(
            fs::read(out.join(format!("{}.index.json", hash(&last_good)))).unwrap(),
            last_good
        );
        assert_eq!(run(&descriptor, &out).unwrap()["unchanged"], true);
        for suffix in ["json", "audit.json", "counts.json"] {
            let bytes = format!("complete {suffix}");
            let name = format!("{}.{}", hash(bytes.as_bytes()), suffix);
            fs::write(out.join(format!("{name}.interrupted.pending")), b"partial").unwrap();
            write_immutable(&out, &Budget::new(), &name, bytes.as_bytes()).unwrap();
            assert_eq!(fs::read(out.join(&name)).unwrap(), bytes.as_bytes());
            assert!(write_immutable(&out, &Budget::new(), &name, b"changed bytes").is_err());
            assert_eq!(fs::read(out.join(&name)).unwrap(), bytes.as_bytes());
        }
    }
    #[test]
    fn sparse_neighborhood_survives_dense_cell_and_many_batches_without_identity_loss() {
        let dir = std::env::temp_dir().join(format!(
            "well-detail-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let rows:Vec<Value>=(0..2307).map(|i|json!({"id":format!("USGS-{i:08}"),"longitude":if i<7{0.001+i as f64*0.00001}else{0.45},"latitude":if i<7{0.001}else{0.45}})).collect();
        let mut old = Vec::new();
        for (i, chunk) in rows.chunks(100).enumerate() {
            let pack =
                json!({"coverage":{"indexed":true},"acquisitions":[{"batch":i}],"records":chunk});
            let bytes = serde_json::to_vec(&pack).unwrap();
            let digest = hash(&bytes);
            let url = format!("{digest}.json");
            write_immutable(&dir, &Budget::new(), &url, &bytes).unwrap();
            old.push(json!({"bounds":[0.0,0.0,0.5,0.5],"url":url,"sha256":digest}));
        }
        let compact = compact_details(&dir, &Budget::new(), old.clone(), false).unwrap();
        assert_eq!(
            compact,
            compact_details(&dir, &Budget::new(), old, false).unwrap()
        );
        assert_eq!(
            compact
                .iter()
                .map(|t| t["count"].as_u64().unwrap())
                .sum::<u64>(),
            2307
        );
        let sparse: Vec<_> = compact
            .iter()
            .filter(|t| {
                let b = &t["bounds"];
                b[0].as_f64().unwrap() < 0.002
                    && b[2].as_f64().unwrap() > 0.0
                    && b[1].as_f64().unwrap() < 0.002
                    && b[3].as_f64().unwrap() > 0.0
            })
            .collect();
        assert_eq!(
            sparse
                .iter()
                .map(|t| t["count"].as_u64().unwrap())
                .sum::<u64>(),
            7
        );
        let mut ids = std::collections::BTreeSet::new();
        for t in &compact {
            let p: Value =
                serde_json::from_slice(&fs::read(dir.join(t["url"].as_str().unwrap())).unwrap())
                    .unwrap();
            for r in p["records"].as_array().unwrap() {
                assert!(ids.insert(r["id"].as_str().unwrap().to_owned()));
            }
        }
        assert_eq!(ids.len(), 2307);
        assert!(compact.iter().any(|t| t["depth"] == 8));
        assert!(compact.iter().all(|t| t["count"].as_u64().unwrap() <= 256));
    }
    #[test]
    fn interrupted_staging_is_unreferenced_and_existing_bytes_are_never_truncated() {
        let dir = std::env::temp_dir().join(format!(
            "well-counts-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let bytes = b"complete count artifact";
        let name = format!("{}.counts.json", hash(bytes));
        fs::write(dir.join(format!("{name}.interrupted.pending")), b"partial").unwrap();
        assert!(!dir.join(&name).exists());
        write_immutable(&dir, &Budget::new(), &name, bytes).unwrap();
        let time = fs::metadata(dir.join(&name)).unwrap().modified().unwrap();
        write_immutable(&dir, &Budget::new(), &name, bytes).unwrap();
        assert_eq!(fs::read(dir.join(&name)).unwrap(), bytes);
        assert_eq!(
            fs::metadata(dir.join(&name)).unwrap().modified().unwrap(),
            time
        );
        assert_eq!(
            fs::read(dir.join(format!("{name}.interrupted.pending"))).unwrap(),
            b"partial"
        );
    }
    #[test]
    fn deterministic_increment_and_failure_preserve_previous_index() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let dir = std::env::temp_dir().join(format!(
            "well-index-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let mut capture: Value = serde_json::from_slice(
            &fs::read(root.join("fixtures/us-groundwater/v1/capture.json")).unwrap(),
        )
        .unwrap();
        for acquisition in capture["acquisitions"].as_array_mut().unwrap() {
            if acquisition["collection"] != "monitoring-locations" {
                acquisition["response"]["features"] = json!([]);
            }
        }
        let mut other = capture.clone();
        for acquisition in capture["acquisitions"].as_array_mut().unwrap() {
            if acquisition["collection"] == "monitoring-locations" {
                acquisition["response"]["features"] =
                    json!(&acquisition["response"]["features"].as_array().unwrap()[..3]);
            }
        }
        for acquisition in other["acquisitions"].as_array_mut().unwrap() {
            if acquisition["collection"] == "monitoring-locations" {
                acquisition["response"]["features"] =
                    json!(&acquisition["response"]["features"].as_array().unwrap()[3..]);
            }
        }
        let a = gev_us_groundwater::bundle(&serde_json::to_vec(&capture).unwrap()).unwrap();
        let b = gev_us_groundwater::bundle(&serde_json::to_vec(&other).unwrap()).unwrap();
        let mut refs = Vec::new();
        for (i, v) in [a, b].iter().enumerate() {
            let bytes = serde_json::to_vec(v).unwrap();
            fs::write(dir.join(format!("{i}.json")), &bytes).unwrap();
            refs.push(json!({"path":format!("{i}.json"),"sha256":hash(&bytes)}));
        }
        let descriptor = dir.join("batch.json");
        let out = dir.join("out");
        fs::write(
            &descriptor,
            serde_json::to_vec(&json!({"candidates":[refs[0]],"snapshot":"test"})).unwrap(),
        )
        .unwrap();
        let first = run(&descriptor, &out).unwrap();
        let current: Value =
            serde_json::from_slice(&fs::read(out.join("index.json")).unwrap()).unwrap();
        let counts_path = out.join(current["aggregates"]["overview"]["url"].as_str().unwrap());
        let counts_bytes = fs::read(&counts_path).unwrap();
        let counts_time = fs::metadata(&counts_path).unwrap().modified().unwrap();
        let good_index = fs::read(out.join("index.json")).unwrap();
        fs::write(
            out.join("interrupted-counts.pending"),
            b"partial uncommitted bytes",
        )
        .unwrap();
        assert_eq!(run(&descriptor, &out).unwrap()["unchanged"], true);
        assert_eq!(
            fs::metadata(&counts_path).unwrap().modified().unwrap(),
            counts_time
        );
        assert_eq!(fs::read(&counts_path).unwrap(), counts_bytes);
        assert_eq!(fs::read(out.join("index.json")).unwrap(), good_index);
        fs::write(&counts_path, b"corrupt existing artifact").unwrap();
        assert!(
            run(&descriptor, &out)
                .unwrap_err()
                .to_string()
                .contains("existing count artifact changed")
        );
        assert_eq!(
            fs::read(&counts_path).unwrap(),
            b"corrupt existing artifact"
        );
        assert_eq!(fs::read(out.join("index.json")).unwrap(), good_index);
        // Restore the deliberately corrupted test fixture for later test cases.
        fs::write(&counts_path, &counts_bytes).unwrap();
        fs::write(
            &descriptor,
            serde_json::to_vec(&json!({"candidates":refs,"snapshot":"test"})).unwrap(),
        )
        .unwrap();
        let second = run(&descriptor, &out).unwrap();
        assert!(
            second["indexedRecords"].as_u64().unwrap() > first["indexedRecords"].as_u64().unwrap()
        );
        let before_union: Value =
            serde_json::from_slice(&fs::read(out.join("index.json")).unwrap()).unwrap();
        let mut overlap = refs[0].clone();
        overlap["overlapPolicy"] = json!("retain-first-with-audit");
        fs::write(&descriptor,serde_json::to_vec(&json!({"candidates":[refs[0],refs[1],overlap],"snapshot":"baseline-plus-efficient","expectedUnique":6})).unwrap()).unwrap();
        assert_eq!(run(&descriptor, &out).unwrap()["indexedRecords"], 6);
        let union: Value =
            serde_json::from_slice(&fs::read(out.join("index.json")).unwrap()).unwrap();
        assert_eq!(union["regions"], before_union["regions"]);
        let counts: Value = serde_json::from_slice(
            &fs::read(out.join(union["aggregates"]["overview"]["url"].as_str().unwrap())).unwrap(),
        )
        .unwrap();
        let mut longitude_sum = 0.0;
        let mut latitude_sum = 0.0;
        for region in union["regions"].as_array().unwrap() {
            let p: Value = serde_json::from_slice(
                &fs::read(out.join(region["url"].as_str().unwrap())).unwrap(),
            )
            .unwrap();
            for record in p["records"].as_array().unwrap() {
                longitude_sum += record["longitude"].as_f64().unwrap();
                latitude_sum += record["latitude"].as_f64().unwrap();
            }
        }
        for level in counts["levels"].as_array().unwrap() {
            let sx: f64 = level["cells"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["center"][0].as_f64().unwrap() * c["count"].as_f64().unwrap())
                .sum();
            let sy: f64 = level["cells"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["center"][1].as_f64().unwrap() * c["count"].as_f64().unwrap())
                .sum();
            assert!((sx - longitude_sum).abs() < 1e-7);
            assert!((sy - latitude_sum).abs() < 1e-7);
            assert_eq!(level["total"], 6);
            assert_eq!(
                level["cells"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| c["count"].as_u64().unwrap())
                    .sum::<u64>(),
                6
            );
        }
        assert_eq!(union["overlapAudit"]["overlapIds"], 3);
        assert_eq!(run(&descriptor, &out).unwrap()["unchanged"], true);
        let mut changed = capture.clone();
        for acq in changed["acquisitions"].as_array_mut().unwrap() {
            if acq["collection"] == "monitoring-locations" {
                acq["response"]["features"][0]["properties"]["monitoring_location_name"] =
                    json!("Synthetic alternate source version");
            }
        }
        let alternate = serde_json::to_vec(
            &gev_us_groundwater::bundle(&serde_json::to_vec(&changed).unwrap()).unwrap(),
        )
        .unwrap();
        fs::write(dir.join("alternate.json"), &alternate).unwrap();
        let alt = json!({"path":"alternate.json","sha256":hash(&alternate),"overlapPolicy":"retain-first-with-audit"});
        fs::write(&descriptor,serde_json::to_vec(&json!({"candidates":[refs[0],refs[1],alt],"snapshot":"explicit-conflict-audit","expectedUnique":6})).unwrap()).unwrap();
        run(&descriptor, &out).unwrap();
        let conflict: Value =
            serde_json::from_slice(&fs::read(out.join("index.json")).unwrap()).unwrap();
        assert_eq!(conflict["regions"], before_union["regions"]);
        assert!(
            conflict["overlapAudit"]["differingSerializedVersions"]
                .as_u64()
                .unwrap()
                >= 1
        );
        let good = fs::read(out.join("index.json")).unwrap();
        fs::write(
            &descriptor,
            serde_json::to_vec(&json!({"candidates":[refs[0],refs[0]],"snapshot":"test"})).unwrap(),
        )
        .unwrap();
        assert!(run(&descriptor, &out).is_err());
        assert_eq!(good, fs::read(out.join("index.json")).unwrap());
        fs::write(dir.join("0.json"), b"tampered").unwrap();
        assert!(run(&descriptor, &out).is_err());
        assert_eq!(good, fs::read(out.join("index.json")).unwrap());
    }
}
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|s| s == "--merge-usgwd") {
        if args.len() != 3 {
            eprintln!("usage: well_index --merge-usgwd <state-indexes.json> <output-directory>");
            std::process::exit(1);
        }
        match federation::run(Path::new(&args[1]), Path::new(&args[2])) {
            Ok(v) => println!("{v}"),
            Err(e) => {
                eprintln!("index refused: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    let historical = args.first().is_some_and(|s| s == "--usgwd");
    let args = if historical { &args[1..] } else { &args[..] };
    if args.len() != 2 && !(historical && args.len() == 3) {
        eprintln!(
            "usage: well_index [--usgwd] <pinned-batches.json | normalized-manifest.json> <output-directory> [historical-deadline-seconds:1..900]"
        );
        std::process::exit(1);
    }
    match if historical {
        let seconds = if args.len() == 3 {
            args[2]
                .to_str()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0)
        } else {
            300
        };
        run_input_with_seconds(Path::new(&args[0]), Path::new(&args[1]), true, seconds)
    } else {
        run(Path::new(&args[0]), Path::new(&args[1]))
    } {
        Ok(v) => println!("{v}"),
        Err(e) => {
            eprintln!("index refused: {e}");
            std::process::exit(1);
        }
    }
}
