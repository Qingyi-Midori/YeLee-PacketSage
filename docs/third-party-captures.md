# Third-party capture files (`wireshark/`)

Status: **not part of the repository.** `wireshark/` is excluded by
`.gitignore` (G5-1). The files below were collected locally as decoder
validation material and must not be published with the project.

## Why the directory is excluded

The captures come from third parties and carry their own licences. The project
licence does not cover them, and several of them contain real traffic (with
decryption keys and personal-data-like payloads). Re-publishing them from this
repository would be a redistribution we have no grant for.

The release tarball never includes this directory: `scripts/package_release.py`
ships only the binary, `INSTALL.md`, the built-in rules and `SHA256SUMS`.
Generated reports under `reports/` may name a `wireshark/` path, but they are
gitignored too.

## Inventory and provenance

| Path | Source | Licence / status |
|---|---|---|
| `TCP/200722_win_scale_examples_anon.pcapng` | Wireshark sample captures (`SampleCaptures` wiki) | Wireshark distribution terms (GPL-2.0-or-later); an anonymised upload |
| `MPTCP/redundant_stream1.pcapng` | Wireshark sample captures | Same as above |
| `MPTCP/iperf-mptcp-0-0.pcap` | Wireshark sample captures | Same as above |
| `HTTP/http-chunked-gzip.pcap` | Wireshark sample captures | Same as above |
| `SSL with decryption keys/ssh_curve25519-aes128-gcm_opensshS.pcapng` | Wireshark sample captures ("SSL with decryption keys") | Same as above; contains a session key for this sample |
| `SSL with decryption keys/sftp_dhgex-sha1_aes128-ctr-reassembledS.pcapng` | Wireshark sample captures | Same as above |
| `Cisco/cdp-BCM1100.cap` | Wireshark sample captures | Same as above |
| `Steam/steam-ihs-discovery.pcap` | Wireshark sample captures | Same as above |
| `PROTOS Test Suite Traffic/c07-sip-r2.cap` | PROTOS Test-Suite (University of Oulu, OUSPG) | Test-suite terms; **unverified for redistribution** |
| `PROTOS Test Suite Traffic/c06-ldapv3-enc-r1.pcap.gz` | PROTOS Test-Suite (University of Oulu, OUSPG) | Test-suite terms; **unverified for redistribution** |

## If these files are ever needed in CI

Do not re-add them to the repository. Generate equivalent traffic with
`scripts/gen_traffic.py`, or download the samples from their upstream source at
run time after re-checking the licence of each file.
