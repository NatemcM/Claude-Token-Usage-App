<script lang="ts">
  import Dashboard from "./components/Dashboard.svelte";
  import Settings from "./components/Settings.svelte";

  type View = "dashboard" | "settings";

  let view = $state<View>("dashboard");
</script>

<div
  class="w-[400px] h-[600px] bg-white dark:bg-gray-900 rounded-xl shadow-2xl overflow-hidden border border-gray-200 dark:border-gray-800 flex flex-col"
>
  {#if view === "dashboard"}
    <Dashboard onSettings={() => (view = "settings")} />
  {:else if view === "settings"}
    <Settings onBack={() => (view = "dashboard")} />
  {/if}
</div>
