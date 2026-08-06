import { useEffect, useState } from "react";
import { List, ActionPanel, Action, Icon } from "@raycast/api";
import { STATUS_BUCKETS } from "./types.ts";
import type { Radar } from "./types.ts";
import { groupByBucket, isStale, agentLabel, bucketTitle } from "./lib/state.ts";
import { readRadar, StateFileMissingError } from "./lib/readProjects.ts";

export default function Command() {
  const [radar, setRadar] = useState<Radar | null>(null);
  const [error, setError] = useState<string | null>(null);

  function load() {
    try {
      setRadar(readRadar());
      setError(null);
    } catch (err) {
      setError(err instanceof StateFileMissingError ? err.message : String(err));
    }
  }

  useEffect(() => {
    load();
  }, []);

  if (error) {
    return (
      <List>
        <List.EmptyView title="No data" description={error} icon={Icon.ExclamationMark} />
      </List>
    );
  }

  if (!radar) {
    return <List isLoading />;
  }

  const grouped = groupByBucket(radar.projects);
  const stale = isStale(radar, new Date());

  return (
    <List navigationTitle={stale ? "Petri — data may be stale" : "Petri"} searchBarPlaceholder="Filter projects…">
      {STATUS_BUCKETS.map((bucket) => {
        const projects = grouped[bucket];
        if (projects.length === 0) return null;
        return (
          <List.Section key={bucket} title={bucketTitle(bucket)} subtitle={String(projects.length)}>
            {projects.map((p) => (
              <List.Item
                key={p.id}
                title={p.name}
                subtitle={agentLabel(p)}
                accessories={[
                  { text: p.git.branch ?? "-" },
                  p.git.is_repo && p.git.is_dirty
                    ? { icon: Icon.Circle, tooltip: "Uncommitted changes" }
                    : {},
                ]}
                actions={
                  <ActionPanel>
                    <Action.Open title="Open in Editor" target={p.path} />
                    <Action.Open title="Open Terminal Here" target={p.path} application="Terminal" />
                    {p.git.github_url ? (
                      <Action.OpenInBrowser title="Open on GitHub" url={p.git.github_url} />
                    ) : null}
                    <Action.CopyToClipboard title="Copy Path" content={p.path} />
                    <Action
                      title="Reload"
                      icon={Icon.ArrowClockwise}
                      shortcut={{ modifiers: ["cmd"], key: "r" }}
                      onAction={load}
                    />
                  </ActionPanel>
                }
              />
            ))}
          </List.Section>
        );
      })}
    </List>
  );
}
