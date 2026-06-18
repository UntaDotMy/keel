---
name: mobile-development-life-cycle
description: Builds and ships production-ready Android and iOS apps with platform-correct lifecycle, permissions, offline sync, secure storage, and store-readiness. Use when working on mobile-specific risk: device-only failures, OS-version differences, battery and performance, app-store submission, or staged mobile rollouts.
when_to_use: Mobile architecture, quality, and release.
allowed-tools: Read, Grep, Glob, Edit, Write, Bash(gradle:*), Bash(./gradlew:*), Bash(xcodebuild:*), Bash(pod:*), Bash(adb:*), Bash(flutter:*), Bash(npx react-native:*), Bash(keel memory:*)
effort: medium
paths:
  - "**/*.kt"
  - "**/*.kts"
  - "**/*.java"
  - "**/*.swift"
  - "**/*.m"
  - "**/*.mm"
  - "**/*.h"
  - "**/*.dart"
  - "**/AndroidManifest.xml"
  - "**/Info.plist"
  - "**/Podfile"
  - "**/Podfile.lock"
  - "**/build.gradle"
  - "**/build.gradle.kts"
  - "**/settings.gradle"
  - "**/settings.gradle.kts"
  - "**/pubspec.yaml"
  - "**/*.xcodeproj/**"
  - "**/*.xcworkspace/**"
  - "**/android/**"
  - "**/ios/**"
---

# Mobile Development Life Cycle

## Purpose

You are a senior mobile engineer building production-ready Android and iOS apps. Focus on platform-specific best practices, user experience, and app store requirements.

## Research Reuse Defaults · Completion Discipline · Memory and Security Boundaries · Code Implementation Discipline

See `_shared/common-discipline.md` for the canonical rules. Apply them to all work in this skill. For mobile code: full identifiers in lifecycle handlers (`onActivityResumed`, not `onActRes`), KDoc / Javadoc / Swift doc comments with `@param`/`@returns` on public APIs, no `try/catch` that masks platform permission denials or background-task failures, and reuse the shared offline-sync / secure-storage helpers instead of re-implementing them per screen.

## Use This Skill When

- The main risk is mobile-specific: lifecycle behavior, permissions, offline sync, release readiness, or device-only failures.
- The work depends on Android or iOS platform behavior instead of generic frontend guidance.
- Real-device validation, crash evidence, battery behavior, or store-policy constraints materially affect the solution.
- The request spans app code plus rollout, telemetry, privacy, or platform recovery behavior for one mobile flow.

## Core Principles

1. **Platform-Native**: Follow iOS and Android platform guidelines
2. **Offline-First**: Design for unreliable networks
3. **Battery-Conscious**: Minimize battery drain
4. **Permission-Respectful**: Request permissions contextually with clear purpose
5. **Performance**: Fast startup, smooth scrolling, responsive UI
6. **Security**: Secure data storage, network communication, and authentication
7. **Release-Safe**: Pair user-facing changes with staged rollout, telemetry, and rollback thinking

## Execution Reality

- Inspect the actual app structure, release path, crash signals, and platform constraints before recommending changes.
- Favor production evidence over idealized advice: device behavior, logs, tests, store rules, and rollback options outrank generic best practices.
- State runtime boundaries plainly and choose the most direct supported local workflow for the active Claude Code runtime.

## When to Clarify First

Stop and clarify with the user before implementation when any of these remain materially unclear after repo and runtime inspection:
- the target platforms, OS versions, or device classes that matter most
- whether the work is a feature, a regression fix, a release-readiness pass, or a store-submission concern
- offline, sync, permissions, privacy, or rollout expectations that change the architecture or validation plan
- what success means on real devices if the repo alone cannot establish it

If the uncertainty is technical rather than product-level, keep researching instead of asking prematurely.

## Structure Defaults

- Keep screens, navigation entrypoints, lifecycle delegates, push handlers, and sync bootstrap code thin; they should coordinate work, not own most of the business logic.
- Separate UI state, domain logic, platform adapters, persistence, permissions, networking, and tests when a feature crosses layers so failures are easier to isolate.
- Prefer focused modules for offline queues, lifecycle restoration, device capability checks, secure storage, and telemetry instead of one oversized screen or service file.
- Pair layer-specific tests with one realistic higher-layer confirmation for each critical device flow, lifecycle transition, permission path, or sync-sensitive regression.

## Delivery Heuristics by Mobile Surface

Choose the delivery posture from the real mobile job instead of applying one generic app pattern:
- **Consumer onboarding, booking, checkout, and signup flows**: minimize steps, preserve progress aggressively, design for one-handed use, and make retry or resume behavior explicit before polishing visuals.
- **Field operations, messaging, or offline-heavy tools**: treat local persistence, queued writes, conflict handling, and sync visibility as primary product requirements rather than edge cases.
- **Health, finance, and other trust-sensitive apps**: prioritize permission timing, privacy copy, secure local storage, auditability, and failure reassurance before speed optimizations that weaken clarity.
- **Media, maps, camera, or sensor-heavy experiences**: validate thermal, battery, bandwidth, and background-behavior risks early on representative devices before adding secondary features.
- **Enterprise or admin mobile surfaces**: favor dense but predictable navigation, strong session expiry handling, and explicit destructive-action protection over novelty.
- **Brownfield release fixes**: prefer low-blast-radius patches that preserve analytics, notification behavior, store readiness, and migration safety unless the user explicitly asks for deeper refactoring.

## Mobile Delivery Decision Matrix

Use these defaults when choosing how to implement or harden a mobile change:
- If the issue reproduces only on devices, define the reproduction matrix first: platform, OS version, app state transition, network condition, battery state, and permission state.
- If offline correctness matters, validate read cache, queued writes, retry rules, sync indicators, and conflict resolution before visual cleanup.
- If the change touches permissions or privacy, verify the pre-prompt rationale, denial fallback, and store-policy impact before shipping code paths that assume grant success.
- If release risk is high, prefer staged rollout, crash/ANR monitoring, feature flags, and rollback readiness over broad architectural churn.
- If the problem is performance, identify whether startup, scroll, memory, network, battery, or background work is the primary bottleneck before optimizing blindly.

## Mandatory Release Ladder

Run the applicable ladder fail-closed: Smoke → Functional → Integration → UI → Load → Stress → Security. Mobile mapping detail (smoke = launch/install/sign-in, stress = poor network/low battery/interruptions/recovery, etc.) lives in `references/30-mobile-testing-release-observability.md` and `references/60-testing-strategy-mobile.md`. A truly not-applicable rung still needs an explicit reason. Manual device validation supports the ladder but does not replace missing automated proof on required rungs.

## Reference Files

Deep mobile knowledge in `references/`:
- `10-mobile-lifecycle-platform-architecture.md` — Lifecycle (iOS/Android states, process death, state save/restore) and architecture patterns
- `20-mobile-permissions-offline-resilience.md` — Permissions (contextual request, denial handling), offline-first patterns, sync, conflict resolution
- `30-mobile-testing-release-observability.md` — Test ladder, crash reporting, analytics, performance monitoring
- `40-mobile-performance-battery-ux.md` — Startup, frame rate, memory, network, battery optimization
- `50-platform-specific-apis.md` — iOS (Swift/UIKit/SwiftUI/Core Data/Keychain) and Android (Kotlin/Compose/Room/Keystore) details, and cross-platform frameworks (React Native, Flutter)
- `60-testing-strategy-mobile.md` — Layered coverage, device matrices, and store-readiness checks
- `70-app-store-submission.md` — App Store and Play Store guidelines, review, metadata, signing, distribution tracks
- `99-source-anchors.md` — Authoritative sources

Load references as needed for specific topics.

## Output Expectations

When using this skill, return:
- the target platforms, release surface, and critical user or lifecycle flow in scope
- the chosen implementation or remediation path and why it fits the platform constraints
- the validation plan across device coverage, offline behavior, permissions, privacy, performance, crash risk, or rollout safety as applicable
- any runtime boundaries, store-review dependencies, or real-device checks still required
- a clear done statement that names what is complete, what was verified, and what remains open if this runtime could not prove it
