#!/usr/bin/env node

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

function parseArguments(argv) {
  const values = {};
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (!argument.startsWith("--")) {
      throw new Error(`unexpected argument: ${argument}`);
    }
    const name = argument.slice(2);
    const value = argv[index + 1];
    if (!value || value.startsWith("--")) {
      throw new Error(`missing value for --${name}`);
    }
    values[name] = value;
    index += 1;
  }
  for (const required of ["baseline", "results", "report", "cargo-exit-code"]) {
    if (!values[required]) {
      throw new Error(`missing required --${required}`);
    }
  }
  return {
    baseline: resolve(values.baseline),
    results: resolve(values.results),
    report: resolve(values.report),
    cargoExitCode: Number(values["cargo-exit-code"]),
  };
}

function readJson(path, label) {
  if (!existsSync(path)) {
    throw new Error(`${label} does not exist: ${path}`);
  }
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    throw new Error(`${label} is not valid JSON: ${error.message}`);
  }
}

function requireFiniteNumber(value, label) {
  const number = Number(value);
  if (!Number.isFinite(number)) {
    throw new Error(`${label} must be a finite number`);
  }
  return number;
}

function readOutcomeCount(resultsRoot, name) {
  const path = resolve(resultsRoot, `${name}.txt`);
  if (!existsSync(path)) {
    throw new Error(`cargo-mutants did not produce ${path}`);
  }
  const entries = readFileSync(path, "utf8")
    .split(/\r?\n/u)
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
  return { count: entries.length, path };
}

function summarizeCounts(resultsRoot) {
  const counts = {};
  for (const name of ["caught", "missed", "timeout", "unviable"]) {
    counts[name] = readOutcomeCount(resultsRoot, name);
  }
  const summary = Object.fromEntries(Object.entries(counts).map(([name, value]) => [name, value.count]));
  const total = Object.values(summary).reduce((sum, count) => sum + count, 0);
  const viable = summary.caught + summary.missed + summary.timeout;
  if (total < 1 || viable < 1) {
    throw new Error(`cargo-mutants produced no viable mutation outcomes: ${JSON.stringify(summary)}`);
  }
  summary.total = total;
  summary.viable = viable;
  summary.mutationScore = (summary.caught / viable) * 100;
  summary.files = Object.fromEntries(Object.entries(counts).map(([name, value]) => [name, value.path]));
  return summary;
}

function baselineSummary(baseline) {
  const summary = baseline?.summary;
  if (summary === null || typeof summary !== "object" || Array.isArray(summary)) {
    throw new Error("mutation baseline is missing summary");
  }
  const result = {};
  for (const name of ["totalMutants", "caught", "missed", "timeout", "unviable", "mutationScore", "regressionThreshold"]) {
    result[name] = requireFiniteNumber(summary[name], `baseline.summary.${name}`);
  }
  const countedTotal = result.caught + result.missed + result.timeout + result.unviable;
  if (countedTotal !== result.totalMutants) {
    throw new Error(`baseline counts do not sum to totalMutants: ${countedTotal} != ${result.totalMutants}`);
  }
  const viable = result.caught + result.missed + result.timeout;
  if (viable < 1) {
    throw new Error("baseline has no viable mutants");
  }
  const calculatedScore = (result.caught / viable) * 100;
  if (Math.abs(calculatedScore - result.mutationScore) > 0.01) {
    throw new Error(`baseline mutationScore is inconsistent with its counts: ${result.mutationScore} != ${calculatedScore.toFixed(2)}`);
  }
  if (result.regressionThreshold < 0 || result.regressionThreshold > 100) {
    throw new Error(`baseline regressionThreshold is outside 0..100: ${result.regressionThreshold}`);
  }
  return { ...result, viable, calculatedScore };
}

function writeReport(path, report) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, `${JSON.stringify(report, null, 2)}\n`);
}

function main() {
  let options;
  try {
    options = parseArguments(process.argv.slice(2));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
    return;
  }

  const report = {
    schemaVersion: 1,
    status: "failed",
    cargoExitCode: options.cargoExitCode,
    baselinePath: options.baseline,
    resultsPath: options.results,
  };
  try {
    if (!Number.isInteger(options.cargoExitCode) || ![0, 2].includes(options.cargoExitCode)) {
      throw new Error(`cargo-mutants returned unsupported exit code ${String(options.cargoExitCode)} (expected 0 or 2)`);
    }
    const baseline = baselineSummary(readJson(options.baseline, "mutation baseline"));
    const observed = summarizeCounts(options.results);
    report.baseline = baseline;
    report.observed = observed;
    report.delta = {
      total: observed.total - baseline.totalMutants,
      caught: observed.caught - baseline.caught,
      missed: observed.missed - baseline.missed,
      timeout: observed.timeout - baseline.timeout,
      unviable: observed.unviable - baseline.unviable,
      mutationScore: Number((observed.mutationScore - baseline.mutationScore).toFixed(2)),
    };
    const failures = [];
    if (observed.mutationScore < baseline.regressionThreshold) {
      failures.push(`mutation score ${observed.mutationScore.toFixed(2)} is below threshold ${baseline.regressionThreshold.toFixed(2)}`);
    }
    if (observed.timeout > baseline.timeout) {
      failures.push(`timeouts increased from ${baseline.timeout} to ${observed.timeout}`);
    }
    if (observed.unviable > baseline.unviable) {
      failures.push(`unviable mutants increased from ${baseline.unviable} to ${observed.unviable}`);
    }
    if (failures.length > 0) {
      report.failures = failures;
      throw new Error(failures.join("; "));
    }
    report.status = "passed";
    writeReport(options.report, report);
    console.log(JSON.stringify(report, null, 2));
  } catch (error) {
    report.error = error.message;
    writeReport(options.report, report);
    console.error(error.stack ?? String(error));
    console.error(`Mutation comparison report: ${options.report}`);
    process.exitCode = 1;
  }
}

main();
