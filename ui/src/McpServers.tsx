import { useCallback, useEffect, useState } from "react";

import {
  addMcpServer,
  editMcpServer,
  importMcpServers,
  mcpImportPreview,
  mcpServers as fetchServers,
  projects as fetchProjects,
  removeMcpServer,
  type McpImportCandidate,
  type McpImportPreview,
  type McpServer,
  type NewMcpServer,
  type Project,
} from "./api";

/**
 * The MCP servers AI Team gives its own agents.
 *
 * A chat does not inherit your global Pi registrations - its seats run with an exclusive
 * configuration so a checkout cannot replace the generated planning and context servers -
 * so a server you use in an ordinary Pi session is absent until a row here says
 * otherwise. This page is that row.
 *
 * Two things it refuses to blur, because blurring them is how a settings page lies:
 *
 * - **Configured is not connected.** Nothing on this page opens a connection or runs a
 *   command. A tick says what a seat would be handed; whether the server answers is
 *   learned when a turn starts.
 * - **A value you type is never shown back.** Stored environment variables and headers
 *   come back as names only, and leaving a field blank keeps what is stored rather than
 *   clearing it - which is why moving a server somewhere else asks you for them again
 *   rather than carrying them across or quietly emptying them.
 */
/// A list, or an empty one.
///
/// This page is a section of Settings, so a route answering something other than the
/// list it promises must not take the whole page down with it - the providers and
/// context sources above are what somebody came here to fix.
function listOf<T>(answer: T[]): T[] {
  return Array.isArray(answer) ? answer : [];
}

export function McpServers() {
  const [scope, setScope] = useState<number | null>(null);
  const [servers, setServers] = useState<McpServer[] | null>(null);
  const [projects, setProjects] = useState<Project[]>([]);
  const [problem, setProblem] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setServers(listOf(await fetchServers(scope)));
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, [scope]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    void (async () => {
      try {
        setProjects(listOf(await fetchProjects()));
      } catch {
        // A project list is how the scope is chosen, not how the page works. Without one
        // the app defaults are still editable, which is the common case anyway.
      }
    })();
  }, []);

  const act = async (what: () => Promise<unknown>) => {
    try {
      await what();
      setProblem(null);
      await load();
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  };

  return (
    <section className="settings__group">
      <h3>MCP servers</h3>
      <p className="faint">
        Agents here do not inherit the MCP servers your own Pi sessions use - a chat runs
        with an exclusive configuration so its generated planning and read-only context
        servers cannot be replaced. Add a server here to give it to them.
      </p>
      <p className="faint">
        Configured is not connected: nothing on this page starts a server. Enabling one
        means a seat may run its command on this machine, or reach its endpoint, during a
        turn.
      </p>
      {problem !== null && <p className="error">{problem}</p>}

      <div className="settings__row">
        <label className="settings__toggle" htmlFor="mcp-scope">
          <span>Applies to</span>
        </label>
        <select
          id="mcp-scope"
          aria-label="server scope"
          value={scope === null ? "" : String(scope)}
          onChange={(event) =>
            setScope(event.target.value === "" ? null : Number(event.target.value))
          }
        >
          <option value="">Every project</option>
          {projects.map((project) => (
            <option key={project.id} value={project.id}>
              {project.name}
            </option>
          ))}
        </select>
        <span className="faint settings__detail">
          {scope === null
            ? "What every project's agents get unless a project overrides it."
            : "This project's own servers, and the defaults they may replace."}
        </span>
      </div>

      {servers === null ? (
        <p className="empty">Reading your MCP servers…</p>
      ) : servers.length === 0 ? (
        <p className="empty">No MCP servers configured yet.</p>
      ) : (
        servers.map((server) => (
          <Server
            key={server.id}
            server={server}
            scoped={scope !== null}
            onAct={act}
          />
        ))
      )}

      <Add scope={scope} onAct={act} />
      <Import scope={scope} onAct={act} />
    </section>
  );
}

/** What a server is, in one line a person can check by eye. */
function endpoint(server: McpServer | McpImportCandidate): string {
  return server.transport === "http"
    ? (server.url ?? "")
    : [server.command ?? "", ...server.args].join(" ");
}

function Server({
  server,
  scoped,
  onAct,
}: {
  server: McpServer;
  scoped: boolean;
  onAct: (what: () => Promise<unknown>) => Promise<void>;
}) {
  return (
    <div className="settings__source">
      <div className="settings__row">
        <label className="settings__toggle">
          <input
            type="checkbox"
            aria-label={`enable ${server.name}`}
            checked={server.enabled}
            onChange={(event) =>
              void onAct(() =>
                editMcpServer(server.id, {
                  rev: server.rev,
                  enabled: event.target.checked,
                }),
              )
            }
          />
          <span>{server.name}</span>
        </label>
        <span className="status" data-status={server.enabled ? "done" : "queued"}>
          {server.enabled ? "enabled" : "off"}
        </span>
        <span className="faint settings__detail mono">{endpoint(server)}</span>
      </div>
      <p className="faint settings__detail">
        {server.project_id === null
          ? scoped
            ? "An app default. Add a server of the same name here to replace it for this project."
            : "An app default, offered to every project."
          : "This project only."}
        {server.origin === "import"
          ? ` Imported copy - ${server.origin_detail}. It does not follow the file it came from.`
          : ""}
        {server.enabled
          ? " Enabled, which is not the same as reachable: a seat finds out when its turn starts."
          : ""}
      </p>
      <Edit server={server} onAct={onAct} />
    </div>
  );
}

/// Parse a replacement map, or say why it cannot be used.
///
/// Blank means "leave what is stored", which is the only sense it can have: the page was
/// never given the values, so it has nothing to send back.
function parseMap(body: string): Record<string, string> | undefined {
  if (body.trim() === "") return undefined;
  const parsed: unknown = JSON.parse(body);
  if (
    parsed === null ||
    typeof parsed !== "object" ||
    Array.isArray(parsed) ||
    !Object.values(parsed).every((value) => typeof value === "string")
  ) {
    throw new Error("Give an object of names to values, like {\"API_KEY\": \"…\"}.");
  }
  return parsed as Record<string, string>;
}

function Edit({
  server,
  onAct,
}: {
  server: McpServer;
  onAct: (what: () => Promise<unknown>) => Promise<void>;
}) {
  const [command, setCommand] = useState(server.command ?? "");
  const [args, setArgs] = useState(server.args.join(" "));
  const [url, setUrl] = useState(server.url ?? "");
  const [tools, setTools] = useState((server.include_tools ?? []).join(" "));
  const [secrets, setSecrets] = useState("");
  const [said, setSaid] = useState<string | null>(null);

  const secretLabel = server.transport === "http" ? "headers" : "environment variables";
  const stored = server.transport === "http" ? server.header_keys : server.env_keys;

  const save = () => {
    let replacement: Record<string, string> | undefined;
    try {
      replacement = parseMap(secrets);
    } catch (error: unknown) {
      setSaid(error instanceof Error ? error.message : String(error));
      return;
    }
    setSaid(null);
    void onAct(async () => {
      await editMcpServer(server.id, {
        rev: server.rev,
        ...(server.transport === "http"
          ? { url, ...(replacement ? { headers: replacement } : {}) }
          : {
              command,
              args: args.split(/\s+/).filter((arg) => arg !== ""),
              ...(replacement ? { env: replacement } : {}),
            }),
        include_tools: tools.split(/\s+/).filter((tool) => tool !== ""),
      });
      setSecrets("");
    });
  };

  return (
    <details className="settings__fallback">
      <summary>Edit {server.name}</summary>
      <div className="projects__add">
        {server.transport === "http" ? (
          <input
            aria-label={`${server.name} url`}
            value={url}
            onChange={(event) => setUrl(event.target.value)}
          />
        ) : (
          <>
            <input
              aria-label={`${server.name} command`}
              value={command}
              onChange={(event) => setCommand(event.target.value)}
            />
            <input
              aria-label={`${server.name} arguments`}
              value={args}
              onChange={(event) => setArgs(event.target.value)}
            />
          </>
        )}
      </div>
      <p className="faint settings__detail">
        Tools this server may offer, separated by spaces. Leave it blank for all of them -
        every tool is then put in front of the model on each turn, so a long list is worth
        trimming.
      </p>
      <div className="projects__add">
        <input
          aria-label={`${server.name} tools`}
          value={tools}
          placeholder="all tools"
          onChange={(event) => setTools(event.target.value)}
        />
      </div>
      <p className="faint settings__detail">
        {stored.length === 0
          ? `No ${secretLabel} stored.`
          : `Stored ${secretLabel}: ${stored.join(", ")}. Values are never shown here.`}{" "}
        Paste a replacement object to change them, or leave this blank to keep them.
      </p>
      {stored.length > 0 && (
        <p className="faint settings__detail">
          Changing the {server.transport === "http" ? "URL" : "command or its arguments"}{" "}
          will not carry these {secretLabel} across unseen - they were given to where this
          server points now. Paste their replacement above with the change, or{" "}
          <span className="mono">{"{}"}</span> to clear them. AI Team will not empty them
          for you.
        </p>
      )}
      <div className="projects__add">
        <input
          type="password"
          aria-label={`${server.name} ${secretLabel}`}
          placeholder={`{"NAME": "value"}`}
          value={secrets}
          autoComplete="off"
          onChange={(event) => setSecrets(event.target.value)}
        />
        <button type="button" className="button" onClick={save}>
          Save
        </button>
        <button
          type="button"
          className="button"
          onClick={() => void onAct(() => removeMcpServer(server.id, server.rev))}
        >
          Remove
        </button>
        {said !== null && <span className="error settings__detail">{said}</span>}
      </div>
    </details>
  );
}

function Add({
  scope,
  onAct,
}: {
  scope: number | null;
  onAct: (what: () => Promise<unknown>) => Promise<void>;
}) {
  const [name, setName] = useState("");
  const [transport, setTransport] = useState<"stdio" | "http">("stdio");
  const [where, setWhere] = useState("");
  const [said, setSaid] = useState<string | null>(null);

  const add = () => {
    const words = where.trim().split(/\s+/).filter((word) => word !== "");
    const [command, ...args] = words;
    if (name.trim() === "" || (transport === "stdio" ? !command : where.trim() === "")) {
      setSaid("Give it a name and something to reach.");
      return;
    }
    setSaid(null);
    const server: NewMcpServer = {
      project_id: scope,
      name: name.trim(),
      transport,
      ...(transport === "http"
        ? { url: where.trim() }
        : { command, args }),
    };
    void onAct(async () => {
      await addMcpServer(server);
      setName("");
      setWhere("");
    });
  };

  return (
    <details className="settings__fallback">
      <summary>Add a server</summary>
      <p className="faint settings__detail">
        Added switched off. Give it its credentials with Edit, then enable it - which is
        the point it may run on this machine.
      </p>
      <div className="projects__add">
        <input
          aria-label="new server name"
          placeholder="chrome-devtools"
          value={name}
          onChange={(event) => setName(event.target.value)}
        />
        <select
          aria-label="new server transport"
          value={transport}
          onChange={(event) => setTransport(event.target.value as "stdio" | "http")}
        >
          <option value="stdio">command</option>
          <option value="http">URL</option>
        </select>
        <input
          aria-label="new server location"
          placeholder={transport === "http" ? "https://…" : "npx chrome-devtools-mcp@latest"}
          value={where}
          onChange={(event) => setWhere(event.target.value)}
        />
        <button type="button" className="button" onClick={add}>
          Add
        </button>
        {said !== null && <span className="error settings__detail">{said}</span>}
      </div>
    </details>
  );
}

/**
 * Copy a definition out of your own Pi configuration.
 *
 * A copy, taken once. Nothing here follows the file afterwards, nothing is brought over
 * because it happened to be there, and what arrives is switched off until you enable it.
 *
 * What you import is the list you are looking at, not that list's names. The preview
 * carries a fingerprint of your files and this scope, and the import quotes it back - so
 * a definition that changed while this page was open is refused whole, and you are asked
 * to look again rather than handed something nobody reviewed.
 */
function Import({
  scope,
  onAct,
}: {
  scope: number | null;
  onAct: (what: () => Promise<unknown>) => Promise<void>;
}) {
  const [preview, setPreview] = useState<McpImportPreview | null>(null);
  const [chosen, setChosen] = useState<string[]>([]);
  const [said, setSaid] = useState<string | null>(null);

  // A preview is only good for the scope it was read against, so changing scope drops it
  // rather than leaving a list on screen that the server would refuse.
  useEffect(() => {
    setPreview(null);
    setChosen([]);
    setSaid(null);
  }, [scope]);

  const look = async () => {
    setSaid(null);
    try {
      const answer = await mcpImportPreview(scope);
      setPreview({
        ...answer,
        candidates: listOf(answer?.candidates),
        problems: listOf(answer?.problems),
      });
      setChosen([]);
    } catch (error: unknown) {
      setSaid(error instanceof Error ? error.message : String(error));
    }
  };

  const found = preview?.candidates ?? null;

  return (
    <details className="settings__fallback">
      <summary>Import from your own Pi configuration</summary>
      <p className="faint settings__detail">
        Reads your Pi MCP files and shows what is in them. Nothing is connected to or run,
        no command is executed and no credential is resolved, and nothing is copied until
        you choose it. What you choose is pinned as a copy - later edits to those files do
        not reach AI Team, and AI Team never writes to them.
      </p>
      <p className="faint settings__detail">
        You are importing this list, not these names. If your Pi files change while this
        is open, the import is refused and nothing is copied - press Look again and choose
        from what you can see. A definition AI Team cannot copy exactly says so instead of
        arriving changed.
      </p>
      <div className="projects__add">
        <button type="button" className="button" onClick={() => void look()}>
          Look
        </button>
        {said !== null && <span className="error settings__detail">{said}</span>}
      </div>
      {preview?.problems.map((problem) => (
        <p key={problem} className="faint settings__detail">
          {problem}
        </p>
      ))}
      {found !== null &&
        (found.length === 0 ? (
          <p className="empty">No MCP servers found in your Pi configuration.</p>
        ) : (
          <>
            {found.map((candidate) => (
              <div key={candidate.name} className="settings__row">
                <label className="settings__toggle">
                  <input
                    type="checkbox"
                    aria-label={`import ${candidate.name}`}
                    disabled={!candidate.importable || candidate.already}
                    checked={chosen.includes(candidate.name)}
                    onChange={(event) =>
                      setChosen((before) =>
                        event.target.checked
                          ? [...before, candidate.name]
                          : before.filter((name) => name !== candidate.name),
                      )
                    }
                  />
                  <span>{candidate.name}</span>
                </label>
                <span className="faint settings__detail mono">{endpoint(candidate)}</span>
                <span className="faint settings__detail">
                  {candidate.already ? "Already configured here." : candidate.note}
                </span>
              </div>
            ))}
            <div className="projects__add">
              <button
                type="button"
                className="button"
                disabled={chosen.length === 0 || preview === null}
                onClick={() =>
                  void onAct(async () => {
                    if (preview === null) return;
                    await importMcpServers(scope, chosen, preview.fingerprint);
                    setPreview(null);
                    setChosen([]);
                  })
                }
              >
                Import {chosen.length === 0 ? "" : chosen.length}
              </button>
            </div>
          </>
        ))}
    </details>
  );
}
