# Security and privacy

Never commit credentials, DPAPI files, environment files, private research,
owner/contact details or downloaded national corpora. Do not put keys in URLs,
arguments, browser code, logs or issues. The transport accepts `USGS_API_KEY` only
as a process environment value and sends it in `X-Api-Key`.

Windows `scripts/usgs-key.ps1 -Action Set` accepts masked local entry and protects
it with current-user/current-machine DPAPI outside the checkout. It refuses an
existing file. `-SecretPath` selects an existing protected file when interactive
and executor profiles differ. Do not copy the secret into this repository.
Other platforms must supply their own secret manager; no portable protected
storage is implemented. Offline commands do not need a credential.

Do not post a secret in a vulnerability report. No private reporting endpoint is
configured yet; coordinate a private reporting route before sharing sensitive
details. Stop rather than bypass access restrictions or change security settings.
