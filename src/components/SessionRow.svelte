<script lang="ts">
  import type { SessionRow } from "../lib/types";
  import { formatDuration, formatIdle } from "../lib/duration";
  import { formatTokens } from "../lib/format";

  interface Props {
    row: SessionRow;
    onRemove: (pid: number) => void;
    busy: boolean;
  }
  let { row, onRemove, busy }: Props = $props();

  let expanded = $state(false);

  // Two granularities on purpose: the dot follows the spec's 5-minute
  // "active" window, while the text gives the precise age (and says
  // "active now" only under a minute). A green dot beside "idle 2m" means
  // recently active, not a contradiction.
  const dotClass = $derived(
    row.state === "stale"
      ? "bg-red-500"
      : row.isActive
        ? "bg-green-500"
        : "bg-amber-500",
  );
</script>

<div class="px-4 py-2.5 border-b border-gray-100 dark:border-gray-800">
  <div class="flex items-start gap-2">
    <span class="mt-1.5 h-2 w-2 shrink-0 rounded-full {dotClass}"></span>

    <div class="min-w-0 flex-1">
      <div class="flex items-baseline justify-between gap-2">
        <span class="truncate text-sm font-medium text-gray-900 dark:text-gray-100">{row.name}</span>
        <span class="shrink-0 text-xs font-mono text-gray-500 dark:text-gray-400">
          {formatTokens(row.tokens)}
        </span>
      </div>

      <div class="truncate text-xs text-gray-500 dark:text-gray-400">
        {row.project}{#if row.gitBranch} · {row.gitBranch}{/if}
      </div>

      <div class="text-xs text-gray-400 dark:text-gray-500">
        {#if row.state === "stale"}
          pid {row.pid} · no running process
        {:else}
          up {formatDuration(row.uptimeSecs ?? 0)} · {formatIdle(row.idleSecs)}
        {/if}
      </div>

      {#if row.agents.length > 0}
        <button
          class="mt-1 text-xs text-blue-600 dark:text-blue-400"
          onclick={() => (expanded = !expanded)}
        >
          {expanded ? "▾" : "▸"} {row.agents.length} agent{row.agents.length === 1 ? "" : "s"}
        </button>
        {#if expanded}
          <div class="mt-1 space-y-1 border-l border-gray-200 pl-2 dark:border-gray-700">
            {#each row.agents as agent (agent.agentId)}
              <div class="flex items-baseline justify-between gap-2 text-xs">
                <span class="truncate text-gray-600 dark:text-gray-300">
                  {agent.agentType ?? "agent"}
                </span>
                <span class="shrink-0 font-mono text-gray-400 dark:text-gray-500">
                  {formatTokens(agent.tokens)} · {formatIdle(agent.idleSecs)}
                </span>
              </div>
            {/each}
            <p class="pt-0.5 text-xs text-gray-400 dark:text-gray-500">
              Agents run inside this session and can't be stopped separately.
            </p>
          </div>
        {/if}
      {/if}
    </div>

    {#if row.removable}
      <button
        class="shrink-0 text-xs text-red-600 disabled:opacity-50 dark:text-red-400"
        disabled={busy}
        onclick={() => onRemove(row.pid)}
        title="Remove this dead session's leftover registration files"
      >
        Clear
      </button>
    {/if}
  </div>
</div>
