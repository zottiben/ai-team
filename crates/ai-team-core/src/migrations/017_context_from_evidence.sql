-- A session's retained context was read from a step's `input` alone, which Pi reports
-- beside `cacheRead` and `cacheWrite` rather than around them (rule 12) - so every Claude
-- seat's session held a token or two, and a ChatGPT seat's a few hundred. The events kept
-- each step's whole usage: the latest step's prompt, `totalTokens - output`, is how full
-- the session was. Deltas are never rows, so every usage recorded is a finished step's. A
-- turn that left none keeps what it had rather than a guess.
UPDATE node_run
   SET context_tokens = (
           SELECT json_extract(e.payload_json, '$.message.usage.totalTokens')
                - COALESCE(json_extract(e.payload_json, '$.message.usage.output'), 0)
             FROM event e
            WHERE e.node_run_id = node_run.id
              AND json_extract(e.payload_json, '$.message.usage.totalTokens') > 0
            ORDER BY e.id DESC
            LIMIT 1
       )
 WHERE EXISTS (
           SELECT 1
             FROM event e
            WHERE e.node_run_id = node_run.id
              AND json_extract(e.payload_json, '$.message.usage.totalTokens') > 0
       );
