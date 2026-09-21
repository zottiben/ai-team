import { describe, expect, it } from "vitest";

import { anyMatches, globMatches, patternsOf, zoneMatches } from "./glob";

/**
 * These are `crates/ai-team-core/src/util.rs`'s own test cases, asserted again here.
 *
 * The point is not to test globbing twice - it is that two implementations of one
 * vocabulary drift, and this is the place it shows. If one of these ever fails, the Rust
 * matcher is right and this file is wrong: that one decides where work is routed, this
 * one only decides what the map lights up.
 */
describe("the glob vocabulary, mirrored from util.rs", () => {
  it("stops a single star at a separator", () => {
    expect(globMatches("ui/src/*.tsx", "ui/src/App.tsx")).toBe(true);
    expect(globMatches("ui/src/*.tsx", "ui/src/views/App.tsx")).toBe(false);
    expect(globMatches("Cargo.toml", "Cargo.toml")).toBe(true);
    expect(globMatches("Cargo.toml", "crates/a/Cargo.toml")).toBe(false);
  });

  it("crosses separators with a double star, and lets it match nothing", () => {
    expect(globMatches("crates/**", "crates/ai-team-core/src/lib.rs")).toBe(true);
    expect(globMatches("crates/**/Cargo.toml", "crates/a/Cargo.toml")).toBe(true);
    expect(globMatches("crates/**/Cargo.toml", "crates/a/b/Cargo.toml")).toBe(true);
    // The case that catches naive implementations: `**/` must collapse to nothing.
    expect(globMatches("crates/**/Cargo.toml", "crates/Cargo.toml")).toBe(true);
    expect(globMatches("crates/**", "ui/src/App.tsx")).toBe(false);
  });

  it("treats a zone as any of its lines, and ignores comments", () => {
    const zone = "# the frontend\nui/**\ncrates/ai-team-ui/**\n\n";
    expect(zoneMatches(zone, "ui/src/App.tsx")).toBe(true);
    expect(zoneMatches(zone, "crates/ai-team-ui/src/lib.rs")).toBe(true);
    expect(zoneMatches(zone, "crates/ai-team-core/src/db.rs")).toBe(false);
    expect(zoneMatches("", "anything")).toBe(false);
    // A comment line must never be read as a pattern.
    expect(zoneMatches("# ui/**", "ui/src/App.tsx")).toBe(false);
  });

  it("matches one character that is not a separator with ?", () => {
    expect(globMatches("a?c", "abc")).toBe(true);
    expect(globMatches("a?c", "a/c")).toBe(false);
    expect(globMatches("a?c", "ac")).toBe(false);
  });

  it("reads the live patterns out of a zone", () => {
    expect(patternsOf("# note\n ui/** \n\ncrates/**\n")).toEqual(["ui/**", "crates/**"]);
    expect(patternsOf("")).toEqual([]);
  });

  it("asks a whole touches list at once", () => {
    // What a slice carries: several globs, any of which claims the path.
    expect(anyMatches(["crates/**", "ui/**"], "ui/src/App.tsx")).toBe(true);
    expect(anyMatches(["crates/**"], "ui/src/App.tsx")).toBe(false);
    expect(anyMatches([], "ui/src/App.tsx")).toBe(false);
  });
});
