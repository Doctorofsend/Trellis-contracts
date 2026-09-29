#!/usr/bin/env node
/**
 * Compares freshly measured Soroban budget metrics against a checked-in
 * baseline and reports the result (Issue #192).
 *
 * Deliberately dependency-free (no external JSON/table library) so it runs
 * on a bare `node` with nothing installed -- matching this repo's other
 * one-off Node scripts (validate-changelog.cjs, verify-deployment.cjs,
 * push-issues.js).
 *
 * Input: a cargo-test log (see scripts/profile-budget.sh) containing lines
 * of the form:
 *
 *   BUDGET_METRIC <key> cpu=<cpu_instructions> mem=<memory_bytes>
 *
 * printed by each contract's `profile_budget` test module (e.g.
 * contracts/aid-contract/src/profile_budget.rs). Those are compared against
 * testing/budget-baseline.json, which has the shape:
 *
 *   {
 *     "metrics": {
 *       "aid.create_aid": { "cpu_instructions": N, "memory_bytes": M },
 *       ...
 *     }
 *   }
 *
 * Usage:
 *   node scripts/compare-budget.cjs --log <path> [--baseline <path>]
 *     [--threshold <percent>] [--update-baseline] [--output <path>]
 *
 * Exit codes: 0 = every metric within threshold (or baseline updated),
 * 1 = at least one metric regressed beyond threshold, 2 = could not run.
 */
"use strict";

const fs = require("fs");
const path = require("path");

function parseArgs(argv) {
  const out = {
    log: null,
    baseline: "testing/budget-baseline.json",
    threshold: 10,
    updateBaseline: false,
    output: null,
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--log") out.log = argv[++i];
    else if (a === "--baseline") out.baseline = argv[++i];
    else if (a === "--threshold") out.threshold = Number(argv[++i]);
    else if (a === "--update-baseline") out.updateBaseline = true;
    else if (a === "--output") out.output = argv[++i];
    else {
      console.error(`unknown option: ${a}`);
      process.exit(2);
    }
  }
  return out;
}

/** Scrapes `BUDGET_METRIC <key> cpu=<n> mem=<n>` lines out of raw text. */
function parseMetrics(text) {
  const metrics = {};
  const re = /^BUDGET_METRIC\s+(\S+)\s+cpu=(\d+)\s+mem=(\d+)\s*$/;
  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim();
    const m = re.exec(line);
    if (!m) continue;
    const [, key, cpu, mem] = m;
    metrics[key] = {
      cpu_instructions: Number(cpu),
      memory_bytes: Number(mem),
    };
  }
  return metrics;
}

function readJson(filePath) {
  const abs = path.resolve(filePath);
  if (!fs.existsSync(abs)) return null;
  return JSON.parse(fs.readFileSync(abs, "utf8"));
}

/** Percent change of `current` relative to `base`, or null if base is 0. */
function pctChange(base, current) {
  if (base === 0) return null;
  return ((current - base) / base) * 100;
}

function fmtPct(p) {
  if (p === null) return "n/a";
  const sign = p > 0 ? "+" : "";
  return `${sign}${p.toFixed(2)}%`;
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  if (!args.log) {
    console.error("usage: compare-budget.cjs --log <path> [--baseline <path>] [--threshold <percent>] [--update-baseline]");
    process.exit(2);
  }
  if (!fs.existsSync(args.log)) {
    console.error(`✗ log file not found: ${args.log}`);
    process.exit(2);
  }
  if (!Number.isFinite(args.threshold) || args.threshold < 0) {
    console.error(`✗ invalid --threshold: must be a non-negative number`);
    process.exit(2);
  }

  const current = parseMetrics(fs.readFileSync(args.log, "utf8"));

  if (args.output) {
    fs.writeFileSync(args.output, JSON.stringify({ metrics: current }, null, 2) + "\n");
  }

  if (args.updateBaseline) {
    const baselinePayload = {
      description:
        "Baseline Soroban CPU-instruction / memory-byte costs for the contract calls profiled by scripts/profile-budget.sh (Issue #192). Regenerate with `./scripts/profile-budget.sh --update-baseline` and record why in the PR.",
      generated_at: new Date().toISOString().slice(0, 10),
      threshold_percent_default: 10,
      metrics: current,
    };
    fs.mkdirSync(path.dirname(path.resolve(args.baseline)), { recursive: true });
    fs.writeFileSync(path.resolve(args.baseline), JSON.stringify(baselinePayload, null, 2) + "\n");
    console.log(`✓ wrote ${Object.keys(current).length} metric(s) to ${args.baseline}`);
    process.exit(0);
  }

  const baselineDoc = readJson(args.baseline);
  if (!baselineDoc || typeof baselineDoc.metrics !== "object") {
    console.error(
      `✗ could not read a { "metrics": {...} } baseline from ${args.baseline}. ` +
        `Run "./scripts/profile-budget.sh --update-baseline" once to create it.`
    );
    process.exit(2);
  }
  const baseline = baselineDoc.metrics;

  const keys = Array.from(new Set([...Object.keys(baseline), ...Object.keys(current)])).sort();

  const rows = [];
  let anyRegression = false;
  let anyMissing = false;

  for (const key of keys) {
    const base = baseline[key];
    const now = current[key];

    if (!now) {
      rows.push({ key, status: "MISSING", detail: "not measured this run" });
      anyMissing = true;
      continue;
    }
    if (!base) {
      rows.push({
        key,
        status: "NEW",
        detail: `cpu=${now.cpu_instructions} mem=${now.memory_bytes} (no baseline yet)`,
      });
      continue;
    }

    const cpuPct = pctChange(base.cpu_instructions, now.cpu_instructions);
    const memPct = pctChange(base.memory_bytes, now.memory_bytes);
    const regressed =
      (cpuPct !== null && cpuPct > args.threshold) || (memPct !== null && memPct > args.threshold);
    if (regressed) anyRegression = true;

    rows.push({
      key,
      status: regressed ? "FAIL" : "PASS",
      baseCpu: base.cpu_instructions,
      nowCpu: now.cpu_instructions,
      cpuPct,
      baseMem: base.memory_bytes,
      nowMem: now.memory_bytes,
      memPct,
    });
  }

  const lines = [];
  lines.push(`## Soroban Budget Profile (threshold: ${args.threshold}%)`);
  lines.push("");
  lines.push("| Call | CPU (baseline → now, Δ) | Memory (baseline → now, Δ) | Status |");
  lines.push("| --- | --- | --- | --- |");
  for (const r of rows) {
    if (r.status === "MISSING") {
      lines.push(`| \`${r.key}\` | - | - | ⚠️ MISSING (${r.detail}) |`);
    } else if (r.status === "NEW") {
      lines.push(`| \`${r.key}\` | - | - | 🆕 NEW (${r.detail}) |`);
    } else {
      const icon = r.status === "FAIL" ? "❌ FAIL" : "✓ PASS";
      lines.push(
        `| \`${r.key}\` | ${r.baseCpu} → ${r.nowCpu} (${fmtPct(r.cpuPct)}) | ${r.baseMem} → ${r.nowMem} (${fmtPct(r.memPct)}) | ${icon} |`
      );
    }
  }
  lines.push("");
  if (anyRegression) {
    lines.push(
      `❌ One or more calls exceeded the ${args.threshold}% regression threshold. If the increase is intended, ` +
        `regenerate the baseline with \`./scripts/profile-budget.sh --update-baseline\`, commit ` +
        `\`${args.baseline}\`, and explain why in the PR description.`
    );
  } else {
    lines.push(`✓ No call exceeded the ${args.threshold}% regression threshold.`);
  }
  if (anyMissing) {
    lines.push(
      "",
      "⚠️ Metrics marked MISSING were not measured this run (their contract's `profile_budget` " +
        "tests did not build/run -- see the job log). This does not fail the check; it means that " +
        "call's footprint was not verified this run."
    );
  }
  const report = lines.join("\n");

  console.log(report);

  const summaryPath = process.env.GITHUB_STEP_SUMMARY;
  if (summaryPath) {
    fs.appendFileSync(summaryPath, report + "\n");
  }

  process.exit(anyRegression ? 1 : 0);
}

main();
