# Fixes from the second two-computer test (0.24.2)

A list only. Owner's instruction (2026-11-18): don't start fixing until the logs from both
computers are in; then read the logs, add what they show, and fix in order of harm.

## Seen during set-up

1. **"Adding…" forever.** On the primary's Show network page, *Add to the show* on a waiting
   display computer turned grey ("Adding…") and never finished, with no message. Likely cause:
   Windows Firewall blocking newq.exe's incoming connections on the laptop (the new version was
   in a new folder, so Windows treats it as a new program; its "allow access" prompt can hide
   behind the full-screen waiting card). Beacons go out, so the primary sees the laptop, but the
   primary's dial in (`hub.rs` `NetAction::Adopt` -> `pending` + redial) fails silently.
   Fix: after ~10 s without a session, say so on the row ("Can't reach Assistant at
   192.168.x.x"), name the firewall when the computer is Windows, and log each failed dial
   with its error. Confirm the cause from the logs first.
2. **Display computer: the firewall prompt is hidden behind the full-screen card.** Consider the
   card saying "Not reached by a primary yet: Windows may be blocking NewQ" when beacons go out
   but no primary has connected for a while, or checking for an inbound firewall rule at start
   (needs care: no admin rights).
3. **The release page said the Mac app was "signed and notarised"** when 0.24.2's Mac download
   was ad-hoc signed (`mac_unsigned`). The notes text should follow what was built.
4. **The `mac_unsigned` release option is only on the session branch** (commit "Release
   workflow: a choice to build the Mac download ad-hoc signed"). It needs a PR to `main`; the
   branch first needs restarting from `main` (one force-push of the session branch, owner's
   go-ahead needed).
5. **Number each screen's card on a display computer** (owner, 2026-11-18). Every output's
   waiting card now looks the same; each should say which screen it is (e.g. "Assistant ·
   Screen 1", "Screen 2"), even when the computer has only one, matching the numbering the
   primary's Test mode *Show on* list and Displays menu use for that computer's monitors.
   (`app/play.rs` draws `card`/`card_text` per monitor; Identify already flashes the name.)
6. Optional: rebuild 0.24.2's Mac download signed and notarised once Apple's servers are back.

## Known before this test (still open)

- A display computer claimed by a primary that is gone can't be freed without deleting
  `node.json`; proposed a "leave the network" key.
- A sound output naming the old primary stays with it after a takeover.

## Testing workflow (not NewQ)

- Driving two laptops through other Claude sessions didn't work well: cross-session messages
  were refused (trusted-device check) or never arrived, and every step needed approval. Running
  the test by hand and sending the one zip is simpler: the network test already gathers each
  computer's log into the primary's zip.

## From the logs

(to fill in when the zips arrive)
