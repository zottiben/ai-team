export interface VuState { level: number; peak: number; held: number }
export const ratio = (value: number) => Number.isFinite(value) ? Math.max(0, Math.min(1, value)) : 0;

/** dt is seconds of visible animation, not wall time spent in a hidden tab. */
export function advanceVu(state: VuState, input: number, dt: number): VuState {
  input = ratio(input);
  dt = Math.max(0, dt);
  const level = input > state.level ? input : Math.max(input, state.level - 0.8 * dt);
  if (level >= state.peak) return { level, peak: level, held: 1 };
  const decay = Math.max(0, dt - state.held);
  return { level, peak: Math.max(level, state.peak - 0.6 * decay), held: Math.max(0, state.held - dt) };
}
