import type { ComponentProps } from "react";
import { SynthProvider } from "../../SynthProvider";
import { PresetSelector } from "../../primitives/PresetSelector";
import "../../tokens.css";
import "../../primitives.css";
import "./composer.css";

/** Only picker decoration changes. Chat keeps the form, textarea, images and requests. */
export default function ComposerDeck(props: ComponentProps<typeof PresetSelector>) {
  return <SynthProvider><PresetSelector {...props}/></SynthProvider>;
}
