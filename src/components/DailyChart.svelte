<script lang="ts">
  import { onMount } from "svelte";
  import * as echarts from "echarts/core";
  import { BarChart } from "echarts/charts";
  import { GridComponent, TooltipComponent } from "echarts/components";
  import { SVGRenderer } from "echarts/renderers";
  import { graphic } from "echarts/core";
  import { formatTokens } from "../lib/format";
  import type { DailyTokens } from "../lib/types";

  echarts.use([BarChart, GridComponent, TooltipComponent, SVGRenderer]);

  interface Props {
    data: DailyTokens[];
  }

  let { data }: Props = $props();

  let chartEl: HTMLDivElement | undefined = $state();
  let chart: echarts.ECharts | undefined;

  onMount(() => {
    if (!chartEl) return;

    chart = echarts.init(chartEl, undefined, { renderer: "svg" });

    return () => {
      chart?.dispose();
    };
  });

  $effect(() => {
    if (!chart || !data.length) return;

    const isDark = window.matchMedia("(prefers-color-scheme: dark)").matches;

    chart.setOption({
      grid: {
        left: 8,
        right: 8,
        top: 8,
        bottom: 20,
        containLabel: false,
      },
      tooltip: {
        trigger: "axis",
        formatter: (params: any) => {
          const p = params[0];
          const date = p.name;
          const day = date.split("-")[2];
          const month = date.split("-")[1];
          return `<b>${month}/${day}</b><br/>${formatTokens(p.value)} tokens`;
        },
        backgroundColor: isDark ? "#1f2937" : "#fff",
        borderColor: isDark ? "#374151" : "#e5e7eb",
        textStyle: {
          color: isDark ? "#f3f4f6" : "#111827",
          fontSize: 11,
        },
      },
      xAxis: {
        type: "category",
        data: data.map((d) => d.date),
        axisLabel: {
          show: true,
          fontSize: 9,
          color: isDark ? "#6b7280" : "#9ca3af",
          formatter: (val: string) => val.split("-")[2],
        },
        axisLine: { show: false },
        axisTick: { show: false },
      },
      yAxis: {
        type: "value",
        show: false,
      },
      series: [
        {
          type: "bar",
          data: data.map((d) => d.tokens),
          itemStyle: {
            color: new graphic.LinearGradient(0, 0, 0, 1, [
              { offset: 0, color: "#8b5cf6" },
              { offset: 1, color: "#a78bfa" },
            ]),
            borderRadius: [3, 3, 0, 0],
          },
          barMaxWidth: 16,
        },
      ],
    });
  });
</script>

<div>
  <h3 class="text-xs font-medium text-gray-500 dark:text-gray-400 uppercase tracking-wide mb-2">
    Daily Usage
  </h3>
  <div
    bind:this={chartEl}
    class="w-full h-[120px] bg-gray-50 dark:bg-gray-800 rounded-lg"
  ></div>
</div>
