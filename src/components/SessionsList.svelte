<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { listSessions, removeStaleRegistration } from "../lib/api";
  import type { SessionRow as Row } from "../lib/types";
  import SessionRow from "./SessionRow.svelte";

  interface Props {
    onSettings: () => void;
  }
  let { onSettings }: Props = $props();

  const POLL_MS = 2000;

  let rows = $state<Row[]>([]);
  // Two error slots on purpose: a successful poll every 2s would otherwise
  // erase the "pid X is alive, refusing" message before it could be read.
  let pollError = $state<string | null>(null);
  let actionError = $state<string | null>(null);
  let loading = $state(true);
  let busyPid = $state<number | null>(null);
  let timer: ReturnType<typeof setInterval> | null = null;

  const live = $derived(rows.filter((r) => r.state === "live"));
  const stale = $derived(rows.filter((r) => r.state === "stale"));

  async function refresh() {
    try {
      rows = await listSessions();
      pollError = null;
    } catch (e) {
      pollError = String(e);
    } finally {
      loading = false;
    }
  }

  function start() {
    if (timer !== null) return;
    timer = setInterval(refresh, POLL_MS);
  }

  function stop() {
    if (timer !== null) {
      clearInterval(timer);
      timer = null;
    }
  }

  function onVisibility() {
    // The popover hides on focus loss; don't probe processes while unseen.
    if (document.visibilityState === "visible") {
      refresh();
      start();
    } else {
      stop();
    }
  }

  onMount(() => {
    refresh();
    start();
    document.addEventListener("visibilitychange", onVisibility);
    window.addEventListener("focus", onVisibility);
    window.addEventListener("blur", stop);
  });

  onDestroy(() => {
    stop();
    document.removeEventListener("visibilitychange", onVisibility);
    window.removeEventListener("focus", onVisibility);
    window.removeEventListener("blur", stop);
  });

  async function remove(pid: number) {
    busyPid = pid;
    actionError = null;
    try {
      await removeStaleRegistration(pid);
      await refresh();
    } catch (e) {
      // Survives subsequent polls; cleared on the next attempt.
      actionError = String(e);
    } finally {
      busyPid = null;
    }
  }
</script>

<!--
  Sessions gets its own header rather than putting a gear in the tab bar:
  Dashboard already owns a header with its refresh and settings buttons, and a
  second gear in the tab strip would sit right beside Dashboard's own. This
  keeps Dashboard untouched.
-->
<div class="flex items-center justify-between border-b border-gray-200 px-4 py-3 dark:border-gray-700">
  <div>
    <h1 class="text-sm font-semibold text-gray-900 dark:text-white">Sessions</h1>
    <p class="text-[10px] text-gray-400 dark:text-gray-500">
      {live.length} live{#if stale.length > 0} · {stale.length} stale{/if}
    </p>
  </div>
  <button
    onclick={onSettings}
    class="rounded-md p-1.5 transition-colors hover:bg-gray-100 dark:hover:bg-gray-800"
    title="Settings"
    aria-label="Settings"
  >
    <svg class="h-4 w-4 text-gray-500 dark:text-gray-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
      <path stroke-linecap="round" stroke-linejoin="round" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
      <path stroke-linecap="round" stroke-linejoin="round" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
    </svg>
  </button>
</div>

<div class="flex-1 overflow-y-auto">
  {#if actionError}
    <p class="px-4 py-2 text-xs text-red-600 dark:text-red-400">{actionError}</p>
  {/if}
  {#if pollError}
    <p class="px-4 py-2 text-xs text-amber-600 dark:text-amber-400">{pollError}</p>
  {/if}

  {#if loading}
    <p class="px-4 py-3 text-xs text-gray-500 dark:text-gray-400">Looking for sessions…</p>
  {:else if rows.length === 0}
    <p class="px-4 py-3 text-xs text-gray-500 dark:text-gray-400">
      No Claude Code sessions registered.
    </p>
  {:else}
    {#if live.length > 0}
      <h3 class="px-4 pt-3 pb-1 text-xs font-medium uppercase tracking-wide text-gray-500 dark:text-gray-400">
        Live ({live.length})
      </h3>
      {#each live as row (row.pid)}
        <SessionRow {row} onRemove={remove} busy={busyPid === row.pid} />
      {/each}
    {/if}

    {#if stale.length > 0}
      <h3 class="px-4 pt-3 pb-1 text-xs font-medium uppercase tracking-wide text-gray-500 dark:text-gray-400">
        Stale registrations ({stale.length})
      </h3>
      <p class="px-4 pb-1 text-xs text-gray-400 dark:text-gray-500">
        These sessions are gone but left files behind.
      </p>
      {#each stale as row (row.pid)}
        <SessionRow {row} onRemove={remove} busy={busyPid === row.pid} />
      {/each}
    {/if}
  {/if}
</div>
