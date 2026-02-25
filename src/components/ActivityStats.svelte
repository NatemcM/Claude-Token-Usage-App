<script lang="ts">
  import { formatNumber } from "../lib/format";

  interface Props {
    messages: number;
    sessions: number;
    toolCalls: number;
    totalSessions: number;
    totalMessages: number;
  }

  let { messages, sessions, toolCalls, totalSessions, totalMessages }: Props = $props();
</script>

<div>
  <h3 class="text-xs font-medium text-gray-500 dark:text-gray-400 uppercase tracking-wide mb-2">
    Activity This Month
  </h3>
  <div class="grid grid-cols-3 gap-2">
    <div class="bg-gray-50 dark:bg-gray-800 rounded-lg px-3 py-2">
      <p class="text-[10px] text-gray-400 dark:text-gray-500">Messages</p>
      <p class="text-sm font-semibold text-gray-900 dark:text-white">
        {formatNumber(messages)}
      </p>
    </div>
    <div class="bg-gray-50 dark:bg-gray-800 rounded-lg px-3 py-2">
      <p class="text-[10px] text-gray-400 dark:text-gray-500">Sessions</p>
      <p class="text-sm font-semibold text-gray-900 dark:text-white">
        {formatNumber(sessions)}
      </p>
    </div>
    <div class="bg-gray-50 dark:bg-gray-800 rounded-lg px-3 py-2">
      <p class="text-[10px] text-gray-400 dark:text-gray-500">Tool Calls</p>
      <p class="text-sm font-semibold text-gray-900 dark:text-white">
        {formatNumber(toolCalls)}
      </p>
    </div>
  </div>

  <div class="mt-2 flex gap-4 text-[10px] text-gray-400 dark:text-gray-500">
    <span>All time: {formatNumber(totalMessages)} messages</span>
    <span>{formatNumber(totalSessions)} sessions</span>
  </div>
</div>
