⏺ I'll do a thorough analysis of the codebase. Let me explore different areas in parallel.

⏺ 5 Explore agents finished (ctrl+o to expand)
   ├─ Analyze driver code for duplication · 33 tool uses · 70.1k tokens
   │  ⎿  Done
   ├─ Analyze core modules for issues · 28 tool uses · 55.8k tokens
   │  ⎿  Done
   ├─ Analyze server/API code for issues · 36 tool uses · 68.3k tokens
   │  ⎿  Done
   ├─ Analyze tests for duplication/gaps · 27 tool uses · 89.1k tokens
   │  ⎿  Done
   └─ Analyze config/state/types for issues · 23 tool uses · 74.6k tokens
      ⎿  Done

⏺ Coop Codebase Analysis: Tech Debt, Duplication, and Potential Bugs

  Here's a consolidated analysis across the entire codebase, organized by severity.

  ---
  Critical / High Severity

  1. Possible panic in screen buffer bridge (screen.rs:124)
  - Bridge array is [0u8; 15] but max index needed is esc_len(7) + take(8) = 15, which overflows a 0-14 index range. Could panic on copy_from_slice with specific escape sequences.

  2. Non-atomic error state writes (session.rs:248-256)
  - Setting Error state requires updating 3 separate RwLocks (agent_state, error_detail, error_category). A reader can observe agent_state == Error while error_detail is still None.
   These should be behind a single lock or written atomically.

  3. Silent base64 failure in WebSocket (transport/ws.rs:471)
  - unwrap_or_default() on decode silently sends empty bytes to the PTY. HTTP correctly returns a 400 error for the same case. This is an inconsistency across transports that could
  cause hard-to-debug issues.

  4. Incomplete UTF-8 tail scan (screen.rs:54-77)
  - Only scans last 3 bytes for incomplete sequences, but a 4-byte UTF-8 sequence (U+10000+) needs a scan of 4. Could corrupt non-Latin text at chunk boundaries.

  5. Atomic ordering mismatch (run.rs:236 vs session.rs:104)
  - child_pid is stored with Release but loaded with Relaxed in the closure passed to AppState. Should use Acquire to match the release-acquire pair used everywhere else.

  ---
  Medium Severity — Duplication / DRY

  6. Nearly identical HookDetector implementations (driver/claude/stream.rs:33-115 vs driver/gemini/detect.rs:24-89)
  - ~50+ lines of identical tokio::select! + channel send logic. Differs only in event-to-state mapping. A generic HookDetector<F> parameterized by a mapping function would
  eliminate this.

  7. Byte-for-byte identical NudgeEncoder (claude/encoding.rs:9-31 vs gemini/encoding.rs:9-31)
  - Exact same implementation. Should be pub type GeminiNudgeEncoder = ClaudeNudgeEncoder.

  8. Duplicated StdoutDetector (claude/stream.rs:190-236 vs gemini/detect.rs:96-134)
  - Same JsonlParser feed loop. Claude's version additionally stores last_message — could be an optional feature on a shared generic.

  9. Duplicated Backend::run() logic (pty/spawn.rs:100-181 vs pty/adapter.rs:114-203)
  - NativePty and TmuxBackend have identical tokio::select! branches for Write, Drain, and resize handling. Should extract to a shared helper.

  10. Stop/Start state parallel implementations (stop.rs:131-164 vs start.rs:61-85)
  - Identical broadcast + atomic sequence counter pattern. Both have emit() methods with the same infrastructure but slightly different payloads.

  11. PromptContext boilerplate — 8+ construction sites across driver code all manually setting 9+ fields. Needs Default impl + builder methods.

  12. Response type duplication across transports — HTTP, WebSocket, and gRPC each define their own error conversion and response mapping for the same underlying handler results
  (handler.rs types → transport-specific types).

  ---
  Medium Severity — Possible Bugs / Design Issues

  13. gRPC double-validates resize (grpc.rs:221-222 + handler.rs:309-315)
  - gRPC checks cols <= 0 || rows <= 0 before calling handle_resize() which does the same check. HTTP and WS rely solely on the handler. Inconsistent and redundant.

  14. Missing gRPC SendInputRaw — HTTP and WebSocket both support it; gRPC doesn't. Feature gap for gRPC clients.

  15. State sequence lag (session.rs:232-261)
  - state_seq is incremented locally then stored to the atomic later. Between increment and store, readers see stale sequence numbers.

  16. Silent JSON parse failures (driver/jsonl_stdout.rs:26)
  - Non-JSON stdout lines are silently dropped. Could hide agent crashes or protocol changes. Should at least log dropped lines.

  17. emit() failures silently ignored — Throughout http.rs and grpc.rs, stop.emit() and start.emit() return values are discarded with let _ =. If channels close, no diagnostics.

  18. TOCTOU in config updates (http.rs:436-471)
  - Config is read to generate a block reason, then dropped, then the event is emitted. Between read and emit, another thread could change the config, making the emitted event
  stale.

  ---
  Low Severity — Code Quality / Maintenance

  19. Scattered magic numbers
  - Channel capacities: 256, 64, 64 (run.rs:304-307)
  - Terminal size 80x24 repeated 20+ times in tests
  - Ring buffer sizes (65536, 1048576) repeated 15+ times in tests
  - Prompt enrichment: MAX_ATTEMPTS=10, POLL_INTERVAL=200ms (session.rs:526-527)

  20. Raw serde_json::Value for settings/MCP config (config.rs:304,309)
  - No schema validation. Merge logic in merge_settings() silently falls through on unexpected structures.

  21. Inconsistent skip_serializing_if across serde structs
  - Some use skip_serializing_if = "Option::is_none", some just #[serde(default)], causing different round-trip behavior.

  23. Split state across 5 Arc-wrapped structs (TerminalState, DriverState, LifecycleState, StopState, StartState)
  - Manual ordering comments needed to maintain correctness (e.g., "exit_status must be written before agent_state"). A single state machine struct with explicit transition methods
  would be safer.

  24. Test infrastructure duplication
  - test_state() / AppStateBuilder::new() setup repeated 48+ times across 8+ test files
  - PTY channel setup (mpsc::channel(64) × 3) copy-pasted 9 times
  - Same validation tests (resize, nudge) written 3 times across handler_tests, ws_tests, and session_loop

  25. Auth exemption is path-based (auth.rs:93-102)
  - New routes must be manually added to the exemption list. Easy to forget, creating accidental auth requirements or bypasses.

  26. Known flaky test — plan_prompt_not_overwritten_by_permission_prompt in composite_tests.rs uses real timing (50ms/10ms durations) instead of deterministic ordering.

  ---
  Summary
  ┌───────────────┬───────┬──────────────────────────────────────────────────────────┐
  │   Category    │ Count │                       Top Priority                       │
  ├───────────────┼───────┼──────────────────────────────────────────────────────────┤
  │ Possible bugs │ 7     │ Screen bridge bounds, non-atomic error state, WS base64  │
  ├───────────────┼───────┼──────────────────────────────────────────────────────────┤
  │ Duplication   │ 8     │ HookDetector, NudgeEncoder, Backend::run(), test helpers │
  ├───────────────┼───────┼──────────────────────────────────────────────────────────┤
  │ Design issues │ 6     │ Split state structs, sentinel values, auth pattern       │
  ├───────────────┼───────┼──────────────────────────────────────────────────────────┤
  │ Code quality  │ 5     │ Magic numbers, raw JSON values, test infra               │
  └───────────────┴───────┴──────────────────────────────────────────────────────────┘
  The highest-impact improvements would be:
  1. Fix the screen bridge buffer size (1-line fix, prevents potential panic)
  2. Unify HookDetector/StdoutDetector/NudgeEncoder across drivers (~200 LOC saved)
  3. Wrap error-state writes in a single lock to prevent torn reads
  4. Extract shared test utilities to eliminate ~48 instances of duplicated setup

  ---                                                                                                                                                                                
  Transport Audit: HTTP / WebSocket / gRPC
                                                                                                                                                                                     
  Functionality Gaps                                                                                                                                                                 
  ┌───────────────────────────────┬────────────────────────────┬──────────────────────────────┬─────────────────────────────────┐
  │            Feature            │            HTTP            │              WS              │              gRPC               │
  ├───────────────────────────────┼────────────────────────────┼──────────────────────────────┼─────────────────────────────────┤
  │ Health check                  │       /api/v1/health       │              --              │            GetHealth            │
  ├───────────────────────────────┼────────────────────────────┼──────────────────────────────┼─────────────────────────────────┤
  │ Ready probe                   │       /api/v1/ready        │              --              │               --                │
  ├───────────────────────────────┼────────────────────────────┼──────────────────────────────┼─────────────────────────────────┤
  │ Screen (text)                 │    /api/v1/screen/text     │              --              │               --                │
  ├───────────────────────────────┼────────────────────────────┼──────────────────────────────┼─────────────────────────────────┤
  │ Raw input (base64)            │     /api/v1/input/raw      │           InputRaw           │               --                │
  ├───────────────────────────────┼────────────────────────────┼──────────────────────────────┼─────────────────────────────────┤
  │ Output polling (offset+limit) │       /api/v1/output       │ Replay (no limit/pagination) │  StreamOutput (replay+stream)   │
  ├───────────────────────────────┼────────────────────────────┼──────────────────────────────┼─────────────────────────────────┤
  │ Config: stop (get/put)        │    /api/v1/config/stop     │              --              │  GetStopConfig / PutStopConfig  │
  ├───────────────────────────────┼────────────────────────────┼──────────────────────────────┼─────────────────────────────────┤
  │ Config: start (get/put)       │    /api/v1/config/start    │              --              │ GetStartConfig / PutStartConfig │
  ├───────────────────────────────┼────────────────────────────┼──────────────────────────────┼─────────────────────────────────┤
  │ Resolve stop                  │ /api/v1/hooks/stop/resolve │              --              │           ResolveStop           │
  ├───────────────────────────────┼────────────────────────────┼──────────────────────────────┼─────────────────────────────────┤
  │ Prompt events (push)          │             --             │         PromptAction         │               --                │
  └───────────────────────────────┴────────────────────────────┴──────────────────────────────┴─────────────────────────────────┘
  Hooks (/api/v1/hooks/stop, /api/v1/hooks/start) are internal endpoints called from inside the PTY via curl, so their absence from WS/gRPC is by design.

  Naming Differences

  1. Cursor parameter — HTTP: cursor: bool, gRPC: include_cursor: bool
  2. Health terminal size — HTTP nests: terminal: { cols, rows }, gRPC flattens: terminal_cols, terminal_rows
  3. Stop event signal — WS: signal: Option<Value> (inline JSON), gRPC: signal_json: Option<String> (serialized)
  4. Agent state response — WS reuses StateChange message for StateRequest; HTTP/gRPC have dedicated AgentStateResponse / GetAgentStateResponse types

  Behavioral Gaps

  1. Authentication is inconsistent across all three transports

  - HTTP: Bearer token, middleware exempts /api/v1/health, /ws, and hook endpoints.
  - WS: Token via ?token= query param or Auth message. Read-only operations (ScreenRequest, StateRequest, StatusRequest, Replay) skip the require_auth! check — so they work without
  auth even when a token is configured. HTTP requires auth for the equivalent /screen, /status, /agent/state, /output endpoints.
  - gRPC: No authentication at all. CoopGrpc::into_router() at grpc.rs:143 adds no interceptor or auth layer.

  2. WS StateRequest response is incomplete vs HTTP/gRPC GetAgentState

  WS returns a synthetic StateChange (ws.rs:437-452), which is missing three fields that HTTP (http.rs:308-323) and gRPC (grpc.rs:243-267) include:
  - agent (agent type)
  - since_seq (state sequence number — WS returns screen.seq() instead, conflating screen and state seq)
  - detection_tier

  3. WS write operations return no success response

  Input, InputRaw, Keys, Resize, Signal, and Shutdown all return None on success (ws.rs:465-527). HTTP and gRPC return typed responses with bytes_written, delivered, cols/rows, or
  accepted. A WS client has no way to confirm a write succeeded unless an error comes back.

  4. WS ScreenRequest always includes cursor

  snapshot_to_msg at ws.rs:227 hardcodes cursor: Some(snap.cursor). HTTP defaults to cursor: None unless ?cursor=true. gRPC defaults to no cursor unless include_cursor: true.
  There's no way for a WS client to opt out.

  5. WS Replay response lacks pagination metadata

  HTTP OutputResponse (http.rs:77-83) includes next_offset and total_written for pagination. WS Replay just returns Output { data, offset } — no way to know if more data is
  available or where to resume.

  6. Screen format param accepted but ignored everywhere

  HTTP defines ScreenFormat (text/ansi) at http.rs:52-58 and gRPC defines a Format enum in the proto at line 89, but neither implementation reads the value — both just call
  screen.snapshot(). WS has no format parameter at all. This is consistent (all ignore it) but the HTTP/gRPC definitions imply support that doesn't exist.

  7. gRPC StreamState doesn't emit PromptAction or Exit as distinct events

  WS broadcasts PromptAction events (respond source, prompt type, option chosen) and synthesizes a separate Exit message on process exit. gRPC's StreamState only emits
  AgentStateEvent — prompt responses and exit are folded into state transitions. There's no StreamPromptEvents in the proto.


