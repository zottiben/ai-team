import { useRef, useState } from "react";
import { useAnnounce } from "./SynthProvider";
import { Plate } from "./primitives/Plate";
import { Scope } from "./primitives/Scope";
import { Envelope } from "./primitives/Envelope";
import { Wavetable } from "./primitives/Wavetable";
import { Sparkline } from "./primitives/Sparkline";
import { StepPads } from "./primitives/StepPads";
import { CommandPad, type CommandResult } from "./primitives/CommandPad";
import { SampleBuffer } from "./primitives/graph";
import { HwButton } from "./primitives/HwButton";

const LANES = [
  { id: "think", label: "Think", signal: "think" }, { id: "read", label: "Read", signal: "read" },
  { id: "edit", label: "Edit", signal: "edit" }, { id: "bash", label: "Bash", signal: "bash" }, { id: "mcp", label: "MCP", signal: "mcp" },
] as const;
const DIRECTORIES = [
  { name: "src", hits: [{ position: .2, hits: 8 }, { position: .65, hits: 18 }] },
  { name: "tests", hits: [{ position: .5, hits: 12 }] },
  { name: "assets", hits: [] },
];
export function GraphPreview({ working }: { working: boolean }) {
  const [buffer] = useState(() => { const b = new SampleBuffer(); [0, 8, 3, 12, null, 6].forEach(n => b.push(n)); return b; });
  const [revision, setRevision] = useState(0);
  const [retained, setRetained] = useState([1200, 2300, 4200, 5500]);
  const [compactions, setCompactions] = useState<number[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [result, setResult] = useState<CommandResult>();
  const receipts = useRef(0), announce = useAnnounce();
  const samples = buffer.snapshot();
  const steps = Array.from({ length: 8 + revision }, (_, i) => ({ id: `demo-${i + 1}`, lane: LANES[i % LANES.length]!.id,
    label: `Example step ${i + 1}`, durationMs: i === 3 ? null : 150 + i * 60, current: working && i === 7 + revision }));
  return <>
    <div className="synth-preview__controls"><HwButton onClick={() => { buffer.push((revision * 7 + 13) % 24); setRevision(n => n + 1); }}>Append demo sample</HwButton>
      <HwButton onClick={() => { setCompactions(old => [...old, retained.length]); setRetained(old => [...old, 1300]); }}>Record demo compaction</HwButton></div>
    <div className="synth-preview__grid">
      <Plate heading="Scope · explicit samples"><Scope label="Demo throughput" unit="tok/s" samples={samples} read={() => buffer.snapshot()} active={working} window={64} /></Plate>
      <Plate heading="Envelope · recorded markers"><Envelope label="Demo context" retained={retained} compactions={compactions} /></Plate>
      <Plate heading="Wavetable · deterministic data"><Wavetable directories={DIRECTORIES} /></Plate>
      <Plate heading="Sparkline · bounded history"><Sparkline label="Demo activity" unit="events/s" samples={samples} /></Plate>
      <Plate heading="Command pad · no shell">
        <CommandPad label="Simulate result" command="Fixture only — no process runs" running={working} result={result} onActivate={() => {
          const count = ++receipts.current;
          setResult({ id: String(count), exitCode: count % 2 === 1 ? 0 : 1, finishedAt: Date.now() });
        }} />
        <p className="synth-muted">Press to alternate success/error receipts. The LED rests after two seconds; the exit value stays.</p>
      </Plate>
    </div>
    <Plate heading="Step pads · exact selection"><StepPads label="Demo session pattern" lanes={LANES} steps={steps} arriving={revision ? `demo-${8 + revision}` : undefined}
      onSelect={id => { setSelected(id); announce(`Selected ${id}`); }} />
      {selected && <p>Selected: {selected}</p>}
    </Plate>
  </>;
}
