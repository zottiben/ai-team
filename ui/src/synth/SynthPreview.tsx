import { useState } from "react";
import { SynthProvider } from "./SynthProvider";
import { Plate, type PlateTone } from "./primitives/Plate";
import { Lcd } from "./primitives/Lcd";
import { Led, type LedState } from "./primitives/Led";
import { HwButton } from "./primitives/HwButton";
import { Switch } from "./primitives/Switch";
import { TabStrip } from "./primitives/TabStrip";
import { PresetSelector } from "./primitives/PresetSelector";
import { Knob } from "./primitives/Knob";
import { SegmentMeter } from "./primitives/SegmentMeter";
import { VuMeter } from "./primitives/VuMeter";
import { Chase } from "./primitives/Chase";
import { Cable } from "./primitives/Cable";
import "./primitives.css";

const LEVELS = ["Low", "Medium", "High", "Max"];
const TONES: PlateTone[] = ["night", "midnight", "deck", "magenta", "lavender", "ice", "flamingo", "yellow"];
const STATES: LedState[] = ["off", "on", "pulse", "event", "attention"];
const VIEWS = [{ id: "overview", label: "Overview" }, { id: "review", label: "Review" }, { id: "work", label: "Work" }];

/** Isolated, deterministic controls. Never dispatches or reads an agent or a terminal. */
export default function SynthPreview() {
  const [level, setLevel] = useState(1 / 3);
  const [input, setInput] = useState(0.5);
  const [working, setWorking] = useState(false);
  const [event, setEvent] = useState(0);
  const [view, setView] = useState("overview");
  const [patch, setPatch] = useState({ connected: false, event: 0 });
  return <SynthProvider><div className="synth-ui synth-preview">
    <h3>Synth foundation</h3>
    <p className="synth-muted">Preview controls only — not agent settings or live telemetry. Chat screens are unchanged while the new skin is built. No licensed fonts or external assets are loaded.</p>
    <div className="synth-preview__grid">
      <Plate tone="deck" radius={22} heading="Input · bounded settings">
        <div className="synth-preview__controls">
          <Knob label="Demo thinking" value={level} detents={LEVELS} defaultValue={1 / 3} onCommit={setLevel} />
          <Knob label="Demo activity" value={input} onCommit={setInput} />
          <Knob label="Ratio readout" value={input} readOnly />
        </div>
        <p className="synth-muted">Drag vertically · Shift for fine adjustment · wheel or arrow keys · double-click to reset. Commits on release or after 250ms.</p>
      </Plate>
      <Plate tone="night" heading="Motion · explicit test inputs">
        <div className="synth-preview__stack">
          <Switch label="Simulate working" checked={working} onChange={setWorking} />
          <div className="synth-preview__controls">{STATES.map(state => <span key={state}><Led state={state === "pulse" && !working ? "on" : state} eventId={event} /> {state}</span>)}</div>
          <HwButton onClick={() => setEvent(n => n + 1)}>Trigger event flash</HwButton>
          <VuMeter label="Demo activity" value={input} />
          <Chase working={working} />
          <SegmentMeter label="Example edits" value={3} />
        </div>
      </Plate>
      <Plate heading="Navigation · shared selection">
        <div className="synth-preview__stack">
          <TabStrip label="Preview view" tabs={VIEWS.map(v => ({ ...v, id: `preview-${v.id}`, panel: "preview-panel" }))}
            value={`preview-${view}`} onChange={id => setView(id.replace("preview-", ""))} />
          <PresetSelector label="Preview preset" options={VIEWS} value={view} onChange={setView} />
          <Lcd role="tabpanel" id="preview-panel" aria-labelledby={`preview-${view}`} grid>{VIEWS.find(v => v.id === view)?.label} · demo panel</Lcd>
          <div className="synth-preview__controls"><HwButton variant="primary">Primary example</HwButton><HwButton variant="danger">Danger example</HwButton><HwButton variant="comment">Comment example</HwButton></div>
        </div>
      </Plate>
      <Plate tone="night" heading="Cable · connection spring">
        <Lcd>
          <svg className="synth-preview__cable" viewBox="0 0 300 160" aria-hidden="true">
            <Cable from={{ x: 25, y: 25 }} to={{ x: 275, y: 25 }} connected={patch.connected} eventId={patch.event} />
          </svg>
          <p>{patch.connected ? "Connected example" : "Gate closed example"}</p>
          <HwButton onClick={() => setPatch(p => ({ connected: !p.connected, event: p.event + 1 }))}>{patch.connected ? "Disconnect example" : "Connect example"}</HwButton>
        </Lcd>
      </Plate>
    </div>
    <div className="synth-preview__grid">{TONES.map(tone => <Plate key={tone} tone={tone} heading={tone} actions={<Led state="on" bezel size={6} />}>
      <p>Faceplate · {tone}</p><p className="synth-muted">Secondary text remains readable.</p><Lcd>Read-only LCD data</Lcd>
    </Plate>)}</div>
  </div></SynthProvider>;
}
