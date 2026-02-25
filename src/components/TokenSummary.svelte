<script lang="ts">
  import { formatTokens } from "../lib/format";

  interface Props {
    totalTokens: number;
    inputTokens: number;
    outputTokens: number;
    cacheTokens: number;
  }

  let { totalTokens, inputTokens, outputTokens, cacheTokens }: Props = $props();
</script>

<div class="bg-gradient-to-br from-violet-500 to-violet-700 rounded-xl p-4 text-white">
  <div class="mb-3">
    <p class="text-violet-200 text-xs font-medium">All Time Total</p>
    <p class="text-2xl font-bold tracking-tight">{formatTokens(totalTokens)} tokens</p>
  </div>

  <div class="grid grid-cols-3 gap-2">
    <div class="bg-white/15 rounded-lg px-2.5 py-1.5">
      <p class="text-[10px] text-violet-200">Input</p>
      <p class="text-sm font-semibold">{formatTokens(inputTokens)}</p>
    </div>
    <div class="bg-white/15 rounded-lg px-2.5 py-1.5">
      <p class="text-[10px] text-violet-200">Output</p>
      <p class="text-sm font-semibold">{formatTokens(outputTokens)}</p>
    </div>
    <div class="bg-white/15 rounded-lg px-2.5 py-1.5">
      <p class="text-[10px] text-violet-200">Cache</p>
      <p class="text-sm font-semibold">{formatTokens(cacheTokens)}</p>
    </div>
  </div>
</div>
