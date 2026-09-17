# Icons

`mark.svg` is the source. Everything else in here is generated from it and must not be
hand-edited - change the SVG and run `sh regenerate.sh`.

The mark is an org graph: one orchestrator above, three specialists below, joined by the
edges that make it a team rather than three separate agents. It is deliberately
geometric, because the only hard constraint on an app icon is that it still reads at
16px in a dock, and an illustration at that size is a smudge.

Teal, where ai-planner is blue. The two live in the same dock and the same Applications
folder, so a sibling palette that is merely *similar* would be worse than one that is
clearly different.

Only the five files listed in `tauri.conf.json` are shipped: `32x32.png`,
`128x128.png`, `128x128@2x.png`, `icon.icns` and `icon.ico`. `mark.png` and `icon.png`
are the 1024px intermediates the generator works from.

Check the result at 32px before committing. That, not the 1024px version, is the size it
will actually be seen at.
