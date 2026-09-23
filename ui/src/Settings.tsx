import { useCallback, useEffect, useState } from "react";

import { ContextConnect } from "./ContextConnect";
import { SignIn } from "./SignIn";
import {
  setContext,
  setFallback,
  setProvider,
  setToken,
  settings as fetchSettings,
  type ContextSetting,
  type Settings as SettingsData,
} from "./api";
import { type Theme } from "./theme";

/**
 * Everything that used to need a text editor or a CLI.
 *
 * The one rule this page follows throughout: what the machine profile *allows* and what
 * actually *answers* are different facts, and they are shown separately. A provider that
 * is ticked and not signed into fails at dispatch - minutes later, in a run, somewhere
 * else entirely - so a single tick that means both would be a page that cannot warn about
 * the commonest mistake.
 */
export function Settings({
  theme,
  onTheme,
  onChanged,
}: {
  theme: Theme;
  onTheme: (theme: Theme) => void;
  onChanged: () => void;
}) {
  const [data, setData] = useState<SettingsData | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setData(await fetchSettings());
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const act = async (what: () => Promise<unknown>) => {
    try {
      await what();
      await load();
      // The readiness report changes when a provider does, and the health indicator is
      // reading it - so it is told rather than left to notice on its next poll.
      onChanged();
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  };

  if (data === null) {
    return <p className="empty">{problem ?? "Reading your settings…"}</p>;
  }

  return (
    <div className="settings">
      <div className="main__header">
        <h2>Settings</h2>
      </div>
      {problem !== null && <p className="error">{problem}</p>}

      <section className="settings__group">
        <h3>Models</h3>
        <p className="faint">
          ai-team only ever uses subscriptions and local models - never a metered API key.
          Ticking one here says this machine is allowed to use it; whether it answers is a
          separate question, and both are shown.
        </p>

        {data.providers.map((provider) => (
          <div key={provider.provider} className="settings__source">
            <div className="settings__row">
              <label className="settings__toggle">
                <input
                  type="checkbox"
                  aria-label={`allow ${provider.provider}`}
                  checked={provider.allowed}
                  onChange={(event) =>
                    void act(() => setProvider(provider.provider, event.target.checked))
                  }
                />
                <span>{provider.label}</span>
              </label>

              <span
                className="status"
                data-status={
                  !provider.allowed ? "queued" : provider.reachable ? "done" : "failed"
                }
              >
                {!provider.allowed ? "off" : provider.reachable ? "ready" : "not signed in"}
              </span>

              <span className="faint settings__detail">
                {provider.allowed ? provider.detail : provider.how}
              </span>
            </div>
            {/* Only where it is the thing standing in the way: a sign-in button beside a
                provider that already answers has no use but to log somebody out. */}
            {provider.allowed && !provider.reachable && (
              <SignIn provider={provider} onDone={() => void load()} />
            )}
          </div>
        ))}

        <div className="settings__row">
          <span className="faint">
            When a seat's provider is denied, work goes to the first one in this order that
            is allowed. It is never reordered by an outage - that would send work to a
            different account without saying so.
          </span>
        </div>
        <div className="settings__order">
          {data.fallback.map((name, index) => (
            <span key={name} className="settings__rank">
              <span className="faint mono">{index + 1}</span>
              <span>{name}</span>
              <button
                type="button"
                className="button"
                aria-label={`move ${name} up`}
                disabled={index === 0}
                onClick={() => {
                  const order = [...data.fallback];
                  const above = order[index - 1];
                  const here = order[index];
                  if (above === undefined || here === undefined) return;
                  order[index - 1] = here;
                  order[index] = above;
                  void act(() => setFallback(order));
                }}
              >
                ↑
              </button>
            </span>
          ))}
        </div>
      </section>

      <section className="settings__group">
        <h3>Context sources</h3>
        <p className="faint">
          Read-only, and off until this machine says otherwise. Connect in the browser
          through Pi; a ticket reaches the seats that decide what the work is, and designs
          reach the seat that owns the UI.
        </p>
        {data.context.map((source) => (
          <div key={source.source} className="settings__source">
            <div className="settings__row">
              <label className="settings__toggle">
                <input
                  type="checkbox"
                  aria-label={`allow ${source.source}`}
                  checked={source.allowed}
                  onChange={(event) =>
                    void act(() => setContext(source.source, event.target.checked))
                  }
                />
                <span>{source.source}</span>
              </label>
              {source.allowed && !source.oauth_connected && !source.token_set && (
                <span className="status" data-status="failed">
                  not connected
                </span>
              )}
              {source.allowed && source.oauth_connected && (
                <span className="status" data-status="done">
                  connected with OAuth
                </span>
              )}
              {source.allowed && !source.oauth_connected && source.token_set && (
                <span className="status" data-status="done">
                  ready with manual token
                </span>
              )}
              {source.allowed && !source.oauth_connected && source.held === "environment" && (
                <span className="faint settings__detail">
                  from {source.token_env} in this process
                </span>
              )}
            </div>
            {source.allowed && (
              <>
                <ContextConnect source={source} onDone={() => void act(async () => {})} />
                <details className="settings__fallback">
                  <summary>Manual token fallback</summary>
                  <p className="faint settings__detail">
                    Only for a machine that cannot complete browser OAuth. OAuth remains
                    the normal path and tokens are never returned to this page.
                  </p>
                  <Token source={source} onSaved={() => void act(async () => {})} />
                </details>
              </>
            )}
          </div>
        ))}
      </section>

      <section className="settings__group">
        <h3>Appearance</h3>
        <div className="settings__row">
          {(["system", "dark", "light"] as const).map((option) => (
            <button
              type="button"
              key={option}
              className="button"
              aria-current={theme === option}
              onClick={() => onTheme(option)}
            >
              {option}
            </button>
          ))}
        </div>
      </section>

      <section className="settings__group">
        <h3>Where this lives</h3>
        {/* Said plainly, because the file is hand-editable and this page preserves its
            comments precisely so it stays that way. */}
        <p className="faint mono">{data.profile_path}</p>
      </section>
    </div>
  );
}

/**
 * One context source's token.
 *
 * The field is always blank when it loads, and that is the point: nothing on the settings
 * route produces a stored value, so there is nothing to prefill it with. A page that
 * showed a masked token would be a page that had fetched one.
 *
 * A token exported into the server's own process is shown and not edited. The window
 * cannot unset a variable the process was started with, so offering a field that appears
 * to clear it would be offering a button that does nothing.
 */
function Token({ source, onSaved }: { source: ContextSetting; onSaved: () => void }) {
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [said, setSaid] = useState<string | null>(null);

  if (source.held === "environment") {
    return (
      <p className="faint settings__detail">
        Set in the environment, so it is not editable here. Unset {source.token_env} to
        manage it from this page instead.
      </p>
    );
  }

  const send = async (token: string, done: string) => {
    setBusy(true);
    try {
      await setToken(source.source, token);
      setValue("");
      setSaid(done);
      onSaved();
    } catch (error: unknown) {
      setSaid(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      className="projects__add"
      onSubmit={(event) => {
        event.preventDefault();
        if (value.trim() === "") return;
        void send(value.trim(), "Kept on this machine.");
      }}
    >
      <input
        type="password"
        aria-label={`${source.source} token`}
        placeholder={source.token_set ? "replace the stored token" : "paste the token"}
        value={value}
        onChange={(event) => setValue(event.target.value)}
        autoComplete="off"
      />
      <button type="submit" className="button" disabled={busy || value.trim() === ""}>
        {busy ? "Saving…" : "Save"}
      </button>
      {source.token_set && (
        <button
          type="button"
          className="button"
          disabled={busy}
          onClick={() => void send("", "Removed.")}
        >
          Clear
        </button>
      )}
      {said !== null && <span className="faint settings__detail">{said}</span>}
    </form>
  );
}
