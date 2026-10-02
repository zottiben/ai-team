# Unity — stack rules (starter snippet)

Seeds of common Unity footguns. Delete what doesn't apply; add project specifics
(engine version, render pipeline, target platforms) via the interview.

- **`.meta` files travel with their asset.** Every asset and folder has a sibling
  `.meta` holding its GUID. Move/rename/delete assets *through the Unity editor*,
  or keep the `.meta` in lockstep — an orphaned or missing `.meta` breaks every
  reference to it. Always commit `.meta` files.
- **Never hand-merge scene/prefab YAML.** `.unity` and `.prefab` are serialized
  YAML that conflict badly. Reduce merge surface with prefab variants / additive
  scenes; resolve conflicts with UnityYAMLMerge (Smart Merge), never by hand.
- **Assembly definitions (`.asmdef`) are boundaries.** Respect the assembly graph;
  don't introduce reference cycles. Editor-only code must live under an `Editor/`
  folder or an editor-only asmdef — runtime assemblies that reference
  `UnityEditor` fail to build for players.
- **Tests: EditMode vs PlayMode.** EditMode for pure logic (fast, no scene);
  PlayMode for runtime/coroutine/physics behaviour. Use the Unity Test Framework;
  keep tests in a test asmdef referencing `nunit.framework` + `UnityEngine.TestTools`.
- **Serialized state is fragile.** Prefer `[SerializeField] private` over public
  fields. Renaming a serialized field silently drops its saved/inspector value
  unless you add `[FormerlySerializedAs("old")]`. Use `[CreateAssetMenu]`
  ScriptableObjects for data-driven config.
- **Watch the hot path.** No per-frame allocations in `Update`/`FixedUpdate`; cache
  `GetComponent` results and `Camera.main`; avoid LINQ/boxing in hot loops; pool
  frequently spawned objects. Respect the target platform's frame budget.
- **Builds need a licensed, version-matched editor.** The version is pinned in
  `ProjectSettings/ProjectVersion.txt`. The common CI path is GameCI
  (`game-ci/unity-test-runner`, `unity-builder`) with Git LFS, a cached `Library/`,
  and Unity license secrets. Each platform target (WebGL/Steam/console) has its
  own build step and constraints.
- **Guard platform SDK calls.** Wrap Steam/console/store SDK calls so the game
  still runs in the editor and on other platforms (e.g. check the SDK initialised
  before calling it; ship the required id file like `steam_appid.txt`).
- **Editor automation.** If a Unity MCP is available, drive the editor with it
  (enter play mode, run tests, inspect the scene) instead of guessing runtime
  behaviour.

> Other engines: **Godot** (`project.godot`, GDScript/C#, scenes `.tscn`/`.scn`)
> and **Unreal** (`*.uproject`, C++/Blueprints, `Content/` `.uasset`) get their
> own snippets when you start working in them.
