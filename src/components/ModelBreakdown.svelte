<script lang="ts">
  import { formatTokens, formatModelName } from "../lib/format";
  import type { ModelSummary } from "../lib/types";

  interface Props {
    models: ModelSummary[];
  }

  let { models }: Props = $props();

  const maxTokens = $derived(
    models.length > 0 ? Math.max(...models.map((m) => m.totalTokens)) : 1
  );

  const colors = [
    "bg-violet-500",
    "bg-orange-500",
    "bg-emerald-500",
    "bg-sky-500",
    "bg-pink-500",
  ];
</script>

<div>
  <h3 class="text-xs font-medium text-gray-500 dark:text-gray-400 uppercase tracking-wide mb-2">
    By Model
  </h3>
  <div class="space-y-2.5">
    {#each models as model, i}
      <div>
        <div class="flex items-center justify-between mb-1">
          <span class="text-xs font-medium text-gray-700 dark:text-gray-300 truncate">
            {formatModelName(model.model)}
          </span>
          <span class="text-xs text-gray-500 dark:text-gray-400 ml-2 shrink-0">
            {formatTokens(model.totalTokens)}
          </span>
        </div>
        <div class="h-2 bg-gray-100 dark:bg-gray-800 rounded-full overflow-hidden">
          <div
            class="h-full rounded-full transition-all duration-500 {colors[i % colors.length]}"
            style="width: {(model.totalTokens / maxTokens) * 100}%"
          ></div>
        </div>
        <div class="flex gap-3 mt-0.5">
          <span class="text-[10px] text-gray-400">
            In: {formatTokens(model.inputTokens)}
          </span>
          <span class="text-[10px] text-gray-400">
            Out: {formatTokens(model.outputTokens)}
          </span>
          {#if model.cacheReadTokens > 0}
            <span class="text-[10px] text-gray-400">
              Cache: {formatTokens(model.cacheReadTokens)}
            </span>
          {/if}
        </div>
      </div>
    {/each}
  </div>
</div>
