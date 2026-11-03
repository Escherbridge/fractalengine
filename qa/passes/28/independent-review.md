# QA28 independent evidence review

Reviewed the focused verdict and retained evidence separately from its author. This review approves the narrow outcome: the fresh masked invitation was submitted once, the recipient subsequently resolved Viewer access and exposed the shared hierarchy, and shared node content remains missing. It does not approve the full sharing workflow, peer convergence, the full catalog, integrated tests, CI, or release readiness.

The transfer receipt reports `consumed_once`, three commands applied, and input submitted, with readiness explicitly pending actual transport and role confirmation. The immediate recipient snapshot still shows Genesis Verse, the Join dialog, and pending UI work; it is not completion evidence. The later actual Viewer-role line exists in the retained final recipient stdout. Lawful scoped Viewer hierarchy reads return HTTP 200 with `ok: true`. The Owner-only debug wait's 404 means that no Owner token covers the recipient's active view; it is neither evidence of Join failure nor a reason to weaken that floor.

Both hierarchy read artifacts contain the same shared Verse, Fractal, and six Petal IDs on both sides. The Owner has 25 nodes; the recipient has zero. The recipient's Cube read returns HTTP 200 with `ok: false`, so HTTP status alone cannot establish content success. The repeated hierarchy results remain identical. Their filesystem modification times are 05:11:29 UTC and 05:16:43 UTC; the latter is about 19 minutes after the 04:57:25 UTC Viewer-role log. Those modification times support the recorded approximate delay, but the response bodies themselves carry no capture timestamps.

Role and hierarchy admission support the native Join path's progress. They do not independently establish a complete transport transcript, node replication, journal/history sharing, asset availability, or peer convergence. The source investigation and missing-node repair remain separate work.

Independent read-only checks found:

- All 37 listed input files match the verdict's byte counts and SHA-256 values; their JSON/JSONL contents parse to 50 records.
- Owner stdout: 144,026 bytes, SHA-256 `5e17e9848873025451a7f91d844d72dbe948b541c72964516b63f198f821c92e`, 12 WARN records and no ERROR or panic markers. Owner stderr: 78 bytes, SHA-256 `b6cec634a99f6d3c0924fa4ba1380c085b6aade7fa51b8e17091880e15d7d2d9`.
- Recipient stdout: 163,690 bytes, SHA-256 `1f87d00857ef88cf0b6154ef603bd9037d5536a5bc40daecae243f2e65187e75`, 11 WARN records and no ERROR or panic markers. Recipient stderr is empty, SHA-256 `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`.
- Both final identity artifacts report all eight recorded identities absent, and their PID sets match the pair context. This review inspected those recorded results; it performed no new process query or operation.
- Public JSON/JSONL artifacts and retained raw stdout/stderr contain zero JWT-shaped tokens and zero `fractalengine://join/` URI matches under the independent scan. No raw log payload was copied into this review.
- The staged executable is 364,556,288 bytes. Its independently recomputed SHA-256 is `6a9692d1c494e16536eb5503e94d70e0650be4a4bf1bde11f733391d4f60303e`, matching the recorded stage. The launcher's source commit is `c060f50b995c77f941c59df702b05341b75d9496`; this review did not recompute the source fingerprint or build.

No tests, Cargo commands, apps, process operations, network calls, clipboard access, invitation consumption, or credential replay were performed by this reviewer.
