<template>
  <svg class="template-cover" viewBox="0 0 360 172" fill="none" aria-hidden="true" focusable="false">
      <rect x="32" y="50" width="120" height="72" rx="6" class="paper" />
      <circle cx="50" cy="85" r="6" class="arrow" />
      <path d="m54 89 5 5" class="arrow" />
      <text x="68" y="90">keyword</text>
      <path d="M166 86h25m-7-7 7 7-7 7" class="arrow" />
      <rect x="208" y="26" width="114" height="120" rx="6" class="paper" />
      <path d="M208 46h114" class="rule" />
      <circle cx="219" cy="36" r="2" class="dot" />
      <circle cx="227" cy="36" r="2" class="dot" />
      <rect x="226" y="62" width="78" height="18" rx="3" class="field" />
      <rect x="226" y="88" width="78" height="18" rx="3" class="field" />
      <text x="234" y="76">url</text>
      <path d="M234 97h54M234 118h42M234 128h34" class="rule" />
  </svg>
</template>

<style scoped>
.template-cover { display: block; width: 100%; height: auto; background: var(--sidebar); }
.paper { fill: var(--surface); stroke: var(--border-strong); stroke-width: 1.5; }
.rule { stroke: var(--border-strong); stroke-width: 1.5; stroke-linecap: round; }
.field { fill: var(--muted-bg); }
.arrow { stroke: var(--text-2); stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; }
.dot { fill: var(--text-2); }
text { fill: var(--text); font: 500 13px var(--font); }
</style>
