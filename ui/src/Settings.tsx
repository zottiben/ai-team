import { useCallback, useEffect, useState } from "react";

import {
  setContext,
  setFallback,
  setProvider,
  settings as fetchSettings,
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
          <div key={provider.provider} className="settings__row">
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
          Read-only, and off until this machine says otherwise. A ticket reaches the seats
          that decide what the work is; designs reach the seat that owns the UI.
        </p>
        {data.context.map((source) => (
          <div key={source.source} className="settings__row">
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
            {/* Enabled without a token is worth saying here rather than at the first call
                an agent makes. */}
            {source.allowed && !source.token_set && (
              <span className="status" data-status="failed">
                {source.token_env} is not set
              </span>
            )}
            {source.allowed && source.token_set && (
              <span className="status" data-status="done">
                ready
              </span>
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
