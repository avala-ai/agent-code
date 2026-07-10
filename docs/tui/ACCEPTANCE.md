# Modern TUI acceptance (Appendix C)

**Status:** stub — complete on each terminal in SUPPORT.md before default flip (M10).

- [ ] Mode badge visible in every state; Shift+Tab mid-turn changes the next permission decision  
- [ ] Ctrl+C cancels a long tool ≤150 ms; UI returns to Idle; Esc never cancels a turn  
- [ ] Stream a long answer, PgUp mid-stream, read 30 s: viewport never moves; pill counts; End returns  
- [ ] Idle 60 s: no repaints (frame counter), no CPU wakeups (spot-check)  
- [ ] tmux: no flicker during heavy streaming; no focus-seq leak after exit; OSC 52 with passthrough  
- [ ] Permission modal on a long tool input: fully scrollable; allow-session suppresses repeat  
- [ ] Two queued prompts survive MaxTurns and send on demand  
- [ ] Two subagents: pane ordering; attach/detach loses nothing; kill confirms  
- [ ] Tool spill on large bash output: UI stays responsive; open pager restores  
- [ ] Resize storm: no panic, no corruption, correct reflow  
- [ ] Panic injection restores terminal  
- [ ] `--tui classic` unchanged vs main (scripted classic session)  

Record per-terminal results in SUPPORT.md.
