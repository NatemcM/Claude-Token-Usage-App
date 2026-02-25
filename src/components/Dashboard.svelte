<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { getStats, getCurrentMonthPrefix, updateTrayTitle } from "../lib/api";
  import { formatTokens } from "../lib/format";
  import type { StatsCache, ModelSummary, DailyTokens } from "../lib/types";
  import TokenSummary from "./TokenSummary.svelte";
  import DailyChart from "./DailyChart.svelte";
  import ModelBreakdown from "./ModelBreakdown.svelte";
  import ActivityStats from "./ActivityStats.svelte";

  interface Props {
    onSettings: () => void;
  }

  let { onSettings }: Props = $props();

  let stats = $state<StatsCache | null>(null);
  let loading = $state(true);
  let error = $state("");
  let lastRefresh = $state<Date | null>(null);

  // Derived from stats
  let monthPrefix = $state(getCurrentMonthPrefix());
  let totalTokens = $derived.by(() => {
    if (!stats) return 0;
    return Object.values(stats.modelUsage).reduce(
      (sum, m) =>
        sum +
        m.inputTokens +
        m.outputTokens +
        m.cacheReadInputTokens +
        m.cacheCreationInputTokens,
      0,
    );
  });

  let inputTokens = $derived.by(() => {
    if (!stats) return 0;
    return Object.values(stats.modelUsage).reduce(
      (sum, m) => sum + m.inputTokens,
      0,
    );
  });

  let outputTokens = $derived.by(() => {
    if (!stats) return 0;
    return Object.values(stats.modelUsage).reduce(
      (sum, m) => sum + m.outputTokens,
      0,
    );
  });

  let cacheTokens = $derived.by(() => {
    if (!stats) return 0;
    return Object.values(stats.modelUsage).reduce(
      (sum, m) => sum + m.cacheReadInputTokens + m.cacheCreationInputTokens,
      0,
    );
  });

  let modelSummaries = $derived.by((): ModelSummary[] => {
    if (!stats) return [];
    return Object.entries(stats.modelUsage)
      .map(([model, usage]) => ({
        model,
        inputTokens: usage.inputTokens,
        outputTokens: usage.outputTokens,
        cacheReadTokens: usage.cacheReadInputTokens,
        cacheCreationTokens: usage.cacheCreationInputTokens,
        totalTokens:
          usage.inputTokens +
          usage.outputTokens +
          usage.cacheReadInputTokens +
          usage.cacheCreationInputTokens,
      }))
      .sort((a, b) => b.totalTokens - a.totalTokens);
  });

  let dailyTokens = $derived.by((): DailyTokens[] => {
    if (!stats) return [];
    return stats.dailyModelTokens
      .filter((d) => d.date.startsWith(monthPrefix))
      .map((d) => ({
        date: d.date,
        tokens: Object.values(d.tokensByModel).reduce((s, v) => s + v, 0),
      }));
  });

  let monthMessages = $derived.by(() => {
    if (!stats) return 0;
    return stats.dailyActivity
      .filter((d) => d.date.startsWith(monthPrefix))
      .reduce((sum, d) => sum + d.messageCount, 0);
  });

  let monthSessions = $derived.by(() => {
    if (!stats) return 0;
    return stats.dailyActivity
      .filter((d) => d.date.startsWith(monthPrefix))
      .reduce((sum, d) => sum + d.sessionCount, 0);
  });

  let monthToolCalls = $derived.by(() => {
    if (!stats) return 0;
    return stats.dailyActivity
      .filter((d) => d.date.startsWith(monthPrefix))
      .reduce((sum, d) => sum + d.toolCallCount, 0);
  });

  let unlisten: UnlistenFn | null = null;

  onMount(async () => {
    await loadData();
    unlisten = await listen("stats-updated", () => {
      loadData();
    });
  });

  onDestroy(() => {
    unlisten?.();
  });

  async function loadData() {
    loading = true;
    error = "";
    try {
      stats = await getStats();
      lastRefresh = new Date();
    } catch (e: any) {
      error = e?.toString() || "Failed to load stats";
    } finally {
      loading = false;
    }
  }
</script>

<div class="flex flex-col h-full">
  <!-- Header -->
  <div class="flex items-center justify-between px-4 py-3 border-b border-gray-200 dark:border-gray-700">
    <div>
      <h1 class="text-sm font-semibold text-gray-900 dark:text-white">
        Claude Code Usage
      </h1>
      {#if lastRefresh}
        <p class="text-[10px] text-gray-400 dark:text-gray-500">
          Updated {lastRefresh.toLocaleTimeString()}
        </p>
      {/if}
    </div>
    <div class="flex items-center gap-2">
      <button
        onclick={loadData}
        disabled={loading}
        class="p-1.5 rounded-md hover:bg-gray-100 dark:hover:bg-gray-800 transition-colors disabled:opacity-50"
        title="Refresh"
      >
        <svg class="w-4 h-4 text-gray-500 dark:text-gray-400" class:animate-spin={loading} fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
          <path stroke-linecap="round" stroke-linejoin="round" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
        </svg>
      </button>
      <button
        onclick={onSettings}
        class="p-1.5 rounded-md hover:bg-gray-100 dark:hover:bg-gray-800 transition-colors"
        title="Settings"
      >
        <svg class="w-4 h-4 text-gray-500 dark:text-gray-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
          <path stroke-linecap="round" stroke-linejoin="round" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
          <path stroke-linecap="round" stroke-linejoin="round" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
        </svg>
      </button>
    </div>
  </div>

  <!-- Content -->
  <div class="flex-1 overflow-y-auto">
    {#if error}
      <div class="m-4 p-3 text-sm text-red-500 bg-red-50 dark:bg-red-900/20 rounded-lg">
        {error}
      </div>
    {/if}

    {#if loading && !lastRefresh}
      <div class="flex items-center justify-center h-full">
        <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-violet-600"></div>
      </div>
    {:else}
      <div class="p-4 space-y-4">
        <TokenSummary
          {totalTokens}
          {inputTokens}
          {outputTokens}
          {cacheTokens}
        />

        {#if dailyTokens.length > 0}
          <DailyChart data={dailyTokens} />
        {/if}

        {#if modelSummaries.length > 0}
          <ModelBreakdown models={modelSummaries} />
        {/if}

        <ActivityStats
          messages={monthMessages}
          sessions={monthSessions}
          toolCalls={monthToolCalls}
          totalSessions={stats?.totalSessions ?? 0}
          totalMessages={stats?.totalMessages ?? 0}
        />
      </div>
    {/if}
  </div>
</div>
