use std::io::{self, Read, Write};

fn main() {
    if let Err(error) = run() {
        eprintln!("well export refused: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    // No network, environment, credential, database or arbitrary output-file access.
    let mut args = std::env::args().skip(1);
    let first = args.next();
    let sdr_mode = first.as_deref() == Some("--texas-sdr-v1");
    let inventory_state = if first.as_deref() == Some("--inventory-bundle-v1") {
        Some(
            args.next()
                .ok_or("inventory mode requires a two-digit state code")?,
        )
    } else {
        None
    };
    let bundle_mode = matches!(
        first.as_deref(),
        Some("--bundle-v1" | "--bundle-v1-if-changed")
    );
    let unchanged_hash = if first.as_deref() == Some("--bundle-v1-if-changed") {
        args.next()
    } else {
        None
    };
    if (!sdr_mode && !bundle_mode && inventory_state.is_none() && first.is_some())
        || (first.as_deref() == Some("--bundle-v1-if-changed") && unchanged_hash.is_none())
        || unchanged_hash.as_ref().is_some_and(|hash| {
            hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        || args.next().is_some()
    {
        return Err(
            "usage: gev-us-groundwater [--bundle-v1 | --bundle-v1-if-changed <sha256> | --inventory-bundle-v1 <state-code> | --texas-sdr-v1]".into(),
        );
    }
    let mut bytes = Vec::new();
    let byte_limit = if sdr_mode {
        gev_us_groundwater::texas_sdr::MAX_BYTES as u64
    } else {
        gev_us_groundwater::MAX_BYTES
    };
    io::stdin().take(byte_limit + 1).read_to_end(&mut bytes)?;
    let output = if sdr_mode {
        gev_us_groundwater::texas_sdr::normalize(&bytes)?
    } else if let Some(state) = inventory_state {
        gev_us_groundwater::inventory_bundle(&bytes, &state)?
    } else if bundle_mode {
        gev_us_groundwater::bundle(&bytes)?
    } else {
        gev_us_groundwater::export(&bytes)?
    };
    if unchanged_hash.as_deref().is_some_and(|previous| {
        output["manifest"]["input"]["sha256"]
            .as_str()
            .is_some_and(|current| current.eq_ignore_ascii_case(previous))
    }) {
        return Ok(());
    }
    serde_json::to_writer_pretty(io::stdout().lock(), &output)?;
    io::stdout().write_all(b"\n")?;
    Ok(())
}
