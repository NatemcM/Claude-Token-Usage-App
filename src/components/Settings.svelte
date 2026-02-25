<script lang="ts">
  import { onMount } from "svelte";
  import { getStats } from "../lib/api";
  import type { StatsCache } from "../lib/types";

  interface Props {
    onBack: () => void;
  }

  let { onBack }: Props = $props();

  let stats = $state<StatsCache | null>(null);

  onMount(async () => {
    try {
      stats = await getStats();
    } catch {
      // Ignore
    }
  });
</script>

<div class="flex flex-col h-full">
  <!-- Header -->
  <div class="flex items-center gap-3 px-4 py-3 border-b border-gray-200 dark:border-gray-700">
    <button
      onclick={onBack}
      class="p-1 rounded-md hover:bg-gray-100 dark:hover:bg-gray-800 transition-colors"
      title="Back"
    >
      <svg class="w-5 h-5 text-gray-600 dark:text-gray-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
        <path stroke-linecap="round" stroke-linejoin="round" d="M15 19l-7-7 7-7" />
      </svg>
    </button>
    <h2 class="text-sm font-semibold text-gray-900 dark:text-white">Settings</h2>
  </div>

  <!-- Content -->
  <div class="flex-1 overflow-y-auto p-4 space-y-6">
    <!-- Data Source -->
    <div>
      <h3 class="text-xs font-medium text-gray-500 dark:text-gray-400 uppercase tracking-wide mb-3">Data Source</h3>
      <div class="bg-gray-50 dark:bg-gray-800 rounded-lg p-3 space-y-2">
        <div class="flex justify-between items-center">
          <span class="text-sm text-gray-600 dark:text-gray-400">Source</span>
          <span class="text-xs font-mono text-gray-500 dark:text-gray-500">~/.claude/stats-cache.json</span>
        </div>
        <div class="flex justify-between items-center">
          <span class="text-sm text-gray-600 dark:text-gray-400">Status</span>
          <div class="flex items-center gap-1.5">
            {#if stats}
              <div class="w-2 h-2 rounded-full bg-green-500"></div>
              <span class="text-sm text-green-600 dark:text-green-400">Active</span>
            {:else}
              <div class="w-2 h-2 rounded-full bg-red-500"></div>
              <span class="text-sm text-red-500">Not found</span>
            {/if}
          </div>
        </div>
        {#if stats}
          <div class="flex justify-between items-center">
            <span class="text-sm text-gray-600 dark:text-gray-400">Last Updated</span>
            <span class="text-sm text-gray-900 dark:text-white">{stats.lastComputedDate}</span>
          </div>
          <div class="flex justify-between items-center">
            <span class="text-sm text-gray-600 dark:text-gray-400">First Session</span>
            <span class="text-sm text-gray-900 dark:text-white">
              {stats.firstSessionDate?.split("T")[0] ?? "---"}
            </span>
          </div>
        {/if}
      </div>
    </div>

    <!-- Update Behavior -->
    <div>
      <h3 class="text-xs font-medium text-gray-500 dark:text-gray-400 uppercase tracking-wide mb-3">Updates</h3>
      <div class="bg-gray-50 dark:bg-gray-800 rounded-lg p-3 space-y-2">
        <div class="flex justify-between items-center">
          <span class="text-sm text-gray-600 dark:text-gray-400">Method</span>
          <span class="text-sm text-gray-900 dark:text-white">File watcher</span>
        </div>
        <div class="flex justify-between items-center">
          <span class="text-sm text-gray-600 dark:text-gray-400">Fallback</span>
          <span class="text-sm text-gray-900 dark:text-white">Every 60s</span>
        </div>
      </div>
      <p class="text-[10px] text-gray-400 dark:text-gray-500 mt-2">
        The menu bar updates automatically when Claude Code writes new stats.
      </p>
    </div>

    <!-- About -->
    <div>
      <h3 class="text-xs font-medium text-gray-500 dark:text-gray-400 uppercase tracking-wide mb-3">About</h3>
      <div class="bg-gray-50 dark:bg-gray-800 rounded-lg p-3 space-y-2">
        <div class="flex justify-between items-center">
          <span class="text-sm text-gray-600 dark:text-gray-400">Version</span>
          <span class="text-sm text-gray-900 dark:text-white">0.1.0</span>
        </div>
        <div class="flex justify-between items-center">
          <span class="text-sm text-gray-600 dark:text-gray-400">Menu bar</span>
          <span class="text-sm text-gray-900 dark:text-white">Current month tokens</span>
        </div>
      </div>
    </div>
  </div>
</div>
