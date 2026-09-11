<script lang="ts">
  import Dashboard from "./components/Dashboard.svelte";
  import Settings from "./components/Settings.svelte";
  import SessionsList from "./components/SessionsList.svelte";

  type Tab = "usage" | "sessions";

  let tab = $state<Tab>("usage");
  let showSettings = $state(false);

  const tabClass = (active: boolean) =>
    active
      ? "text-gray-900 dark:text-gray-100 border-b-2 border-gray-900 dark:border-gray-100"
      : "text-gray-500 dark:text-gray-400 border-b-2 border-transparent";
</script>

<div
  class="w-[400px] h-[600px] bg-white dark:bg-gray-900 rounded-xl shadow-2xl overflow-hidden border border-gray-200 dark:border-gray-800 flex flex-col"
>
  {#if showSettings}
    <!-- Back returns to whichever tab opened Settings, not always Usage. -->
    <Settings onBack={() => (showSettings = false)} />
  {:else}
    <div class="flex items-center gap-4 border-b border-gray-200 px-4 dark:border-gray-800">
      <button class="py-2 text-sm {tabClass(tab === 'usage')}" onclick={() => (tab = "usage")}>
        Usage
      </button>
      <button class="py-2 text-sm {tabClass(tab === 'sessions')}" onclick={() => (tab = "sessions")}>
        Sessions
      </button>
    </div>

    {#if tab === "usage"}
      <Dashboard onSettings={() => (showSettings = true)} />
    {:else}
      <SessionsList onSettings={() => (showSettings = true)} />
    {/if}
  {/if}
</div>
