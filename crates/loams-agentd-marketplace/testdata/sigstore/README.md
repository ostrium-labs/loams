`cosign-linux-amd64.sigstore.json` is the Sigstore bundle that cosign's own
release workflow published for `cosign-linux-amd64` in
[sigstore/cosign v3.0.6](https://github.com/sigstore/cosign/releases/tag/v3.0.6),
over the binary whose SHA-256 is
`c956e5dfcac53d52bcf058360d579472f0c1d2d9b69f55209e256fe7783f4c74`
(`cosign_checksums.txt` of the same release). `tests/sigstore.rs` verifies it
offline against the embedded public-good trusted root.
