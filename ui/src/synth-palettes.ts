/** IDs are the persisted contract. Colours live only in synth/tokens.css. */
export const SYNTH_PALETTES = [
  { id: "vice-city", name: "Vice City", description: "Neon · original" },
  { id: "gruvbox", name: "Gruvbox", description: "Warm · earthy" },
  { id: "tokyo-night", name: "Tokyo Night", description: "Cool · electric" },
  { id: "catppuccin", name: "Catppuccin", description: "Soft · pastel" },
  { id: "everforest", name: "Everforest", description: "Natural · calm" },
  { id: "kanagawa", name: "Kanagawa", description: "Inky · restrained" },
] as const;

export type SynthPalette = typeof SYNTH_PALETTES[number]["id"];
export function validSynthPalette(value: unknown): value is SynthPalette {
  return SYNTH_PALETTES.some(palette => palette.id === value);
}
