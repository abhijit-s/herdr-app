# Agent harness manifests

How Herdr lists an agent that doesn't report its status over OSC 7501: `[detect]` names its foreground process, and `[screen]` and `[title]` read its state (working, idle, needs input) off the bottom of its screen and its window title. `src/harness.rs` reads them; `defaults/AGENTS.md` describes the fallback.

These are tuios's harness manifests, copied unchanged from https://github.com/Gaurav-Gosain/tuios/tree/b58d85aa1254becefd598cc2640af126a2e49e98/internal/harness/manifests (MIT, `LICENSE-tuios.txt`). Their rules derive from herdr's agent manifests (Apache-2.0, `LICENSE-herdr.txt`). Herdr reads `[detect]`, `[screen]` and `[title]` only; the other sections are tuios's and are ignored.

To add a harness, copy its tuios manifest here (or write one in the same format) and add screens for its rules to `tests/harness-screens`.
