# Rainfolk

### A well-th of information

Rainfolk aims to make public water information easier to find, explore, and trace back to its sources. The planned free public tools include a water-focused interactive map, a source catalog, and research prompts that guide people to original, reliable sources.

Wells are the first collection. The project's geographic focus is nationwide, not limited to Terlingua.

## Current status

This repository is currently a project home with a README. The working collector and data pipeline are being developed locally; their source code is not published here yet. Reusable source code and the God's Eye View integration are being prepared for security and license review before publication.

`rainfolk.org` is the intended home for the project. The public starter site has not launched.

## Current local pipeline

The local implementation uses Rust with deterministic transport scripts. Its current pipeline is:

**collect -> validate -> spatial index -> God's Eye View layer**

The pipeline is still being refined. Collection and processing do not require an AI call for every record. AI may be used during development or research.

Rainfolk is intended to be useful without depending on a founder-operated service. Exportable data, source attribution, and provenance are part of that goal. Data views should make gaps and source, retrieval, and measurement times explicit.

## Coverage and limits

The latest verified USGS inventory snapshot, dated **October 25, 2026**, contains:

- **1,042,777** distinct USGS monitoring-location IDs
- **44** jurisdictions with pagination complete for this source
- **7** jurisdictions with pagination partial for this source
- **9** quarantined records

This USGS inventory is location metadata. No water-level time-series observations were fetched in this collection. Pagination complete describes retrieval progress for this source; it does not mean all wells or relevant records have been found.

Rainfolk is not a complete registry of private wells. Its information does not establish groundwater availability, drilling suitability, or whether water is potable. Consult original sources and qualified local authorities for decisions that require current or site-specific information.

## Contributing source leads and quality fixes

Useful contributions include links to public water-information sources and corrections to source metadata, coverage, or retrieval results. Please include the source URL, the agency or publisher, the area covered, and the date you checked it.

Do not submit private well-owner details, credentials, or other sensitive information.

## Security and licensing

The USGS API key used by the local collector is protected locally. Keys and other secrets must never be committed to this repository or placed in browser code.

This README does not grant a blanket license. Code and data licensing will be documented per component after source rights and third-party terms have been reviewed. Rights to source material remain with their respective rights holders.

## Roadmap

- Review and publish reusable source code and document rights by component.
- Make source attribution, data provenance, coverage gaps, and relevant timestamps clear.
- Develop the nationwide well-focused source catalog and interactive map.
- Prepare the intended public site at `rainfolk.org`.

## Links

- [USGS monitoring-location API documentation](https://api.waterdata.usgs.gov/docs/ogcapi/) - current source documentation.
- [God's Eye View](https://github.com/bilawalsidhu/gods-eye-view) - independent upstream project; no official partnership is implied.
- [Terlingua Water Atlas](https://github.com/Papa-Oso/terlingua-water-atlas) - historical research and catalog snapshot, not Rainfolk's current codebase or a rebranded version of this project.