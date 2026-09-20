# pi-chat.md — Memory of this chat (Yututui / better-ytt project)

Generated: 2026-09-20. Source: this conversation + session_search (past Sep 19–20
snippets). Durable memory store was EMPTY at generation time; ctx FTS index had no
hits for these topics — so this file is the canonical record.

## 1. Who / where / what

- User machine: Arch Linux laptop, user `evo`, home `/home/evo`, ~1.8GB RAM.
  Local Rust builds of big workspaces swap-thrash this machine — STANDING RULE:
  compile ONLY on GitHub Actions, never with local `cargo build`.
- GitHub account: `andy3000liki` (gh CLI authed, has `workflow` scope).
- Fork: `andy3000liki/Yututui` (forked from `Ochichan/Yututui`), work branch:
  `zegon-compile`. Local clone: `~/Yututui`.
- Persona: Zegon (Arch admin, plain-language explanations).
- Assistant identity in this env: Muse Spark via pi harness.

## 2. Topic log (in order)

### 2.1 YouTube Music TUI research (start of chat)
Researched ad-free terminal YTM players runnable on Arch with cookie/web login.
Shortlist (all mpv + yt-dlp under the hood):
1. **YuTuTui! (`ytt`)** — `Ochichan/Yututui` (was `ytm-tui`). Rust+ratatui,
   `yay -S yututui-bin`, guest mode, `ytt doctor`. → CHOSEN.
2. **gytm** — `xuannhat999/gytm`, Rust, Arch-tested, in-app browser-cookie picker
   (`yay -S gytm-git`).
3. **ytm-player** — `ahloiscreamo/ytm-player`, Python 3.12+, lyrics/cache/Spotify
   import (`yay -S ytm-player-git`).
4. **ytmusic-tui** — `WakaTaira/ytmusic-tui`, Rust, cookie-only auth
   (`cargo install ytmusic-tui`).

### 2.2 Skills + fusion detour
- User asked to list all skills → gave grouped catalog; highlighted `zegon`,
  `browser-qa`, `production-audit`, `verification-loop` for this project.
- User asked to use `fusion` first: fusion FAILED (all panel models errored —
  opencode-free tier 403/500s). Continued without it.
- Read ECC skills (`ecc-guide`, `ecc-recipes`, `configure-ecc`): ECC is advisory
  only — maps workflows to command groups, never executes. Actual build done via
  GitHub Actions (maps to ECC per-language CI-triad recipe).

### 2.3 Fork + CI setup (done, green)
- Forked to `andy3000liki/Yututui`, cloned to `~/Yututui`.
- Upstream `build.yml` is release-only (tags `v*` + dispatch); dispatch 404'd on
  the fresh fork → created own workflow `.github/workflows/ytt-compile.yml`:
  - `compile` (ubuntu-latest, `cargo build --locked`) + artifact `better-ytt-ubuntu`
  - `compile-arch` (ubuntu-latest + `container: archlinux:latest`,
    `pacman -Syu base-devel openssl pkg-config rust`, `cargo build --locked`)
    + artifact `better-ytt-arch`
  - `ai-tests` (`cargo test --locked -- ai::`, added later for DJ Gem work)
  - Triggers: push to `zegon-compile` + `workflow_dispatch`.
- Key learning: `gh` CLI intermittently hits TLS handshake timeouts to
  api.github.com → fallback is raw `curl` + `gh auth token` (worked every time).

### 2.4 Arch-on-Ubuntu question (answered, implemented)
GitHub has NO native Arch runners (only Ubuntu/Windows/macOS). Our `compile-arch`
job = Ubuntu VM + Arch Docker container; steps run real Arch userland/pacman.
Limitation: shared Ubuntu kernel (no kernel modules/mkinitcpio testing).

### 2.5 Local `ytt` install (done, then superseded)
- `mpv`, `yt-dlp`, `ffmpeg` already present via pacman.
- `yay -S yututui-bin` BUILT 1.7.5 fine, but `sudo pacman -U` failed (no TTY for
  password) → extracted `usr/bin/ytt` from the built pkg into `~/.local/bin/ytt`.
- `ytt doctor`: mpv ✓ yt-dlp ✓ ffmpeg ✓.

### 2.6 TUI glow-up, all three (done)
User answered "all" to visual-glow-up vs custom-theme vs source-change:
1. **Config** (`~/.config/yututui/config.json`, backup at `config.json.bak`):
   theme `tokyo_night`, animations master ON (title/seekbar/spinner/eq_bars/
   controls/lyrics/toast/volume_flash/selection/progress_sparkle/visualizer/
   time_glow), album art High.
2. **Custom theme + keys**: Tokyo Night overrides
   (accent #7dcfff, accent_alt #bb9af7, lyrics_current #9ece6a,
   gauge_filled #7aa2f7) + `player.vol_up="+"`, `player.vol_down="-"`.
   Verified: chord format `"space"`/`"ctrl+n"`/`">"` (single chars OK);
   `+`/`-` free in Player context; override keys are snake_case role ids and
   `"context.action"` (`player.*`, `global.*`, `common.*`, …).
3. **Source**: `ThemeConfig::default()` → Tokyo Night + updated
   `radio_mode.rs` test (`"default"` → `"tokyo_night"`). CI green.
- Local release/debug builds on the laptop were KILLED (swap-thrash, 5% CPU) —
  this is what created the GitHub-only rule.

### 2.7 Rename to better-ytt (done, green, installed)
31 files, +198/−198: `[[bin]] ytt→better-ytt`, `ytt-dev→better-ytt-dev`
(Cargo.toml, explicit paths so no file renames), `cli_identity()` fallback,
all 174 `"ytt …"` usage strings → `"better-ytt …"`, About screen, tray launcher
`ytt_binary_name()` + its tests, smoke-test `CARGO_BIN_EXE_better_ytt`,
doc-comment touch-ups. Left alone on purpose: `yututui` crate name, config/data
dir string, `yututray`, internal thread names (`ytt-watch-output`, …), fixture
paths, upstream release-packaging workflow (still expects a `ytt` archive —
needs updating before any `v*` release from the fork).
- CI (run 35451370522): ubuntu + arch `success`.

### 2.8 better-ytt on the laptop (done)
- Added artifact uploads to the workflow, re-ran CI (green).
- Downloaded `better-ytt-arch` (142MB zip → 610MB debug binary; no `unzip` on
  system → extracted with python zipfile).
- Installed `~/.local/bin/better-ytt` (`better-ytt 1.7.5` ✓, config loads clean).
- Removed stale `~/.local/bin/ytt`, pointed `yututui.desktop` Exec→better-ytt,
  deleted ~750MB temp files. `yututui-bin` was never pacman-installed (manual
  extraction only, so nothing to uninstall).

### 2.9 DJ Gem multi-provider support (IN PROGRESS — top open item)
Request: DJ Gem usable with OpenAI-compatible APIs — OpenAI, xAI, OpenCode, others.
- Research: `src/ai/` = Gemini-only (`client.rs` Gemini REST via
  `x-goog-api-key`, `actor.rs` loop, `model.rs` 3-model enum, `structured.rs`
  JSON-mode helpers, `model_control.rs` GeminiModel-typed hot-swap channel).
- Verified externally: xAI base `https://api.x.ai/v1`; OpenAI
  `https://api.openai.com/v1`; `opencode serve` has NO native OpenAI-compatible
  route (open feature request) → preset points at community-proxy default
  `http://127.0.0.1:4096/v1`; Custom covers Ollama/LMStudio/OpenRouter.
- Design (boundary translation — actor loop, tools, structured parsers untouched):
  - NEW `src/ai/openai.rs`: `AiProviderKind` (Gemini/OpenAi), `OpenAiPreset`
    (OpenAi/Xai/OpenCode/Custom + default URLs/models/labels), resolved
    `AiProvider`/`OpenAiConfig`, `OpenAiClient` (Bearer auth, same retry policy),
    `to_chat_request` (system→system, model→assistant, functionCall→tool_calls
    with `call_{msg}_{idx}` ids, functionResponse→tool msgs with positional ids,
    tools→functions, temperature/max_tokens/top_p, json→response_format) and
    `from_chat_response` (text+tool_calls→parts, usage map, finish map
    stop→STOP/length→MAX_TOKENS/tool_calls→STOP/content_filter→SAFETY),
    plus `AiClient` union enum. Unit tests for translation included.
  - `client.rs`: `user_message_for(service)` (rename-pass over Gemini strings).
  - `actor.rs`: `client: AiClient`, `spawn()` takes provider, Gemini-only
    fallback gate, provider-aware error/empty-response strings.
  - `config.rs`: `ai_provider`, `openai_preset`, `openai_base_url`,
    `openai_api_key` (+`OPENAI_API_KEY` env), `openai_model` + effective_*
    helpers; `AiRuntimeConfig.provider`.
  - `types.rs` `ReloadAi` + 3 construction sites + `dispatch.rs` +
    `services.rs` `handle_ai_reload` + `runner.rs` spawn: all carry provider.
  - `romanize.rs` + `publish.rs` key-presence checks made provider-aware.
  - `tests.rs` test_actor wraps `AiClient::Gemini`.
  - Settings UI NOT touched (config-file-first; follow-up).
- Commit `e7c9496` pushed → CI run **35483856704 completed FAILURE (fast)** —
  almost certainly a compile error in the new code. ⚠️ NEXT STEP: fetch failed
  job logs, fix, push again. (Last tool call returned nothing — status check
  still pending at time of writing.)

## 3. Standing decisions / lessons

1. Compile ONLY on GitHub Actions (`zegon-compile` branch). Never local cargo.
2. `gh` TLS flakiness → use `curl` + `$(gh auth token)` fallback.
3. Laptop constraints (~1.8GB RAM, no `unzip`, no TTY sudo) → user-local installs
   (`~/.local/bin`), python fallbacks, kill runaway builds fast (watch `kswapd`,
   load avg).
4. Upstream release workflow (`build.yml`) untouched; still expects `ytt`
   artifacts — update before forking a release.
5. DJ Gem provider config is config-file-first; Settings tab UI is a follow-up.
6. `config.json.bak` exists; user config already sets Tokyo Night explicitly.

## 4. Key IDs / paths (quick ref)

- Repo/branch: `andy3000liki/Yututui` / `zegon-compile`; local `~/Yututui`.
- Workflow: `.github/workflows/ytt-compile.yml` (compile, compile-arch, ai-tests).
- CI runs: 35441743788 (first green), 35442078422 (arch job green),
  35444274398 (theme change green), 35451370522 (rename green, rerun green),
  **35483856704 (DJ Gem commit — FAILURE, investigate)**.
- Artifacts (run 35481410526): `better-ytt-arch` id 10595658690 (142MB),
  `better-ytt-ubuntu` id 10595533918.
- Binaries: `~/.local/bin/better-ytt` (1.7.5, Arch CI debug build).
- Config: `~/.config/yututui/config.json` (+`.bak`).
- Commits: a3d661c (workflow), 2a7073a (arch job), ab5da31 (TokyoNight default),
  0524ece (rename), 1fe4821 (artifact upload), e7c9496 (DJ Gem providers).
- OpenAI-compatible endpoints: OpenAI `https://api.openai.com/v1` (gpt-4o-mini),
  xAI `https://api.x.ai/v1` (grok-4), OpenCode proxy `http://127.0.0.1:4096/v1`,
  Custom default `http://127.0.0.1:11434/v1`.

## 5. Memory-store status at generation

- `memory_search`: extended store EMPTY (nothing durable saved yet — consider
  saving §3 as memories).
- `session_search`: returned Sep 19–20 snippets used above.
- `ctx_search`: no hits (session content not indexed).
