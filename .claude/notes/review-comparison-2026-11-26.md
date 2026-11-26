# The two code reviews of 26 November 2026, compared

Reviews: `review-fable-2026-11-26.md` (16 findings: 0 Critical, 3 High, 6 Medium, 7 Low) and
`review-opus-2026-11-26.md` (11 findings: 0 Critical, 0 High, 4 Medium, 7 Low). Both had the same brief,
worked independently, read code only and ran nothing. Both reviewed `main` at 8033753 (0.27.1).

Every finding only one reviewer made, and the one place they contradict, was checked against the
code by the integrating session. "Checked" below means the quoted code is there and the path holds.
Fable R8 was the one finding not checked (see the table).

## Found by both (7)

| Topic | Fable | Opus | Note |
|---|---|---|---|
| Anyone on the LAN can pose as the primary; the passcode doesn't cover it | R3 High | R1 Medium | Same path, same quoted code. Severity differs: see below |
| Recording "Browse…" folder dialog blocks the main thread | R7 Medium | R4 Medium | Identical |
| Network connections: 64 MB per frame before Hello, no connection cap | R9 Medium (plausible) | R6 Low | Opus adds: an allocation failure aborts, it isn't a catchable panic |
| `Command::output()` FFmpeg runs with no limit, no reaper (`load_image` in the play path) | R10 Low | R5 Low | Both break the hard rule "no program NewQ starts outlives it" |
| Media server answers the whole LAN, no passcode | R13 Low | R7 Low | Opus adds: 32 slots can be held by slow readers |
| Actions pinned by moving tags in the release build | R12 Low | R11 Low | Opus adds: `contents: write` for the whole release workflow |
| A show from a newer NewQ in an older one | R4 Medium | R10 Low | Different halves of one problem: see below |

## Found only by Fable (9)

| ID | Sev. | Finding | Checked |
|---|---|---|---|
| R1 | High | Cues that fire each other with no wait (Start A↔B, or a looped run of instant cues) spin `Engine::fire`'s queue for ever: total freeze | Yes: `engine.rs:1865` loop, `2060` Start → `fire`, no limit (timelines have `MAX_JUMPS`) |
| R2 | High | Quit/close mid-show is unguarded; with unsaved changes a blocking dialog freezes pictures | Yes for the path; `confirm_discard` asks only about unsaved changes |
| R5 | Medium | A display whose render-thread restart fails is never retried (dark until reopened) | Yes: `app.rs:3462-3482`, the watchdog skips `thread == None` |
| R6 | Medium | `ndi::runtime_installed()` `dlopen`s NDI in the main program every frame of two panels | Yes: called from `ui.rs:6662` and `10177` |
| R8 | Medium | A video's picture is paced by its sound device; a device that stalls silently freezes it | Not checked (needs hardware) |
| R11 | Low | Closing an output window switches the display off *in the show* (saved by autosave) | Not checked; low |
| R14 | Low | OSC feedback: an old address lookup can overwrite a newer one | Not checked; low |
| R15 | Low | Waveform reader: `thread::spawn`, no limit, zombie on error | Not checked; low |
| R16 | Low | Edits from the last ≤30 s before a Test Mode run are only in memory for the whole run | Not checked; low |

## Found only by Opus (4)

| ID | Sev. | Finding | Checked |
|---|---|---|---|
| R2 | Medium | OSC send cues look up a host *name* on every send, on the one thread MIDI shares, so one slow name delays every lighting/sound GO behind it | Yes: `output.rs:761` `to_socket_addrs()` per message |
| R3 | Medium | No double-GO protection on any input (a feature QLab has) | Yes: nothing in `src/` |
| R8 | Low | Save / Save As while a save is still writing is dropped silently | Yes: `ui.rs:10956` returns with no message |
| R9 | Low | Two `net-keep-show` threads can write `synced-show.tmp` at once | Yes: `app/play.rs:396-404`, a thread per show taken, one temp name |

## Where they disagree

- **Network impersonation: High (Fable) or Medium (Opus).** Same facts. Fable weighs the broken
  promise: someone who sets a passcode expects it to keep strangers out, and it doesn't for this.
  Opus weighs the likelihood: it needs a deliberate attacker on the show LAN and a moment when the
  primary's link is down. *Integrator's view: High.* The passcode exists to stop exactly this, and
  the fix is small.
- **A show from a newer NewQ: both are right about different parts.**
  - Fable says it loads silently and saving drops unknown *fields*. That is correct: no struct keeps
    unknown fields.
  - Fable also says unknown *kinds* read as `Unknown` or the default. That is only true for some
    enums.
  - Opus is right that `CueKind`, `Transition`, `EffectKind`, `Fit` and `ContinueMode` have no
    fallback (`git grep 'serde(other)' src/show.rs` finds nothing). So one new cue kind makes the
    whole show unreadable, on disk and over the network, where the follower retries every 3 s.
  - CLAUDE.md's hard rule ("most in a show read leniently") is weaker in practice than it reads.
- **Overall tone.** Fable calls R1 close to Critical. Opus says "nothing Critical or High in what I
  read". The difference is coverage, not judgement: Opus didn't read the fire queue or control cues,
  and Fable did.

## What neither reviewed deeply

Both said so plainly, and this is the real residual risk:
- the engine's fades, crossfades, groups and timeline logic;
- the GPU compositor, render threads and uploads;
- the video readers' frame timing;
- the network clock, play sync and `net_correct`;
- the media fetch;
- Mac- and Windows-only code;
- a lock-order (deadlock) audit.

Nothing was run.

## Integrator's ranking of what to fix

1. Fable R1: the cue-loop freeze. A total freeze from an ordinary-looking show.
2. Fable R3 / Opus R1: the passcode not guarding commands.
3. Opus R2: the OSC host lookup delaying GOs.
4. Fable R2: an unguarded quit, and blocking dialogs on the main thread. Do it together with Fable R7 /
   Opus R4: all three are "the main thread waits on a person".
5. Opus R10 + Fable R4: the show format's version skew. Add `saved_with`, and lenient `CueKind` and the
   small enums.
6. Fable R5: retry a display's render thread. Fable R6: no `dlopen` of NDI outside the helper.
7. Opus R3: double-GO protection, as a setting.
8. The Lows, in a batch.
