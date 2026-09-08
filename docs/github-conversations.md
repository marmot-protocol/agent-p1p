# GitHub mentions and feedback

This feature uses the existing signed webhook spool, Rust ledger, Hermes Kanban
board and controller-owned comment publisher. There is no additional daemon or
Hermes fork. It is **opt-in** (`conversations_enabled: true` in repository policy);
omitting the setting preserves existing behavior. Source tests are not proof of
a successful live rollout.

## Behavior

- An explicit mention of Pip's current GitHub login can start a discussion on any
  issue or PR in the configured repository. Numeric identity, not the display
  name, remains authoritative. This does not create a case or authorize a build.
- Human comments/review summaries on a Pip case's PR are assessed even without a
  mention. Questions, nonblocking suggestions and blocking feedback all count.
- On an issue waiting for clarification, a trusted human answer is assessed.
  Inline replies to Pip and a human issue comment immediately following Pip's
  comment also qualify. Issue comments have no explicit reply-parent field;
  otherwise use an explicit mention to remove ambiguity.
- Only configured `intake.trusted_actor_ids` can trigger work. Pip itself, its
  reviewer identities and bot accounts are ignored. Quoted/code mentions do not
  trigger a task. The live comment must still match the signed delivery.
- The conversation worker recommends either an answer alone or a planning
  follow-up. It never edits source, runs builds, publishes to GitHub or merges.
  The controller posts concise Markdown as Pip, without machine-data blobs.
- A follow-up requires an existing case, fresh label authorization, an unchanged
  plan/head, and a supported safe workflow state. It sends retained human feedback
  to a new planning pass; it does not directly dispatch a builder or approve scope.
  Subsequent builders and reviewers receive that feedback alongside normal evidence.
- Completed, abandoned, taken-over, blocked and operationally escalated work is
  not reopened by comments. Exceptional recovery remains an operator action.

## Queue and failure behavior

Comments enter a separate durable inbox in ledger schema 13. Delivery IDs and
live comment versions deduplicate input. Task definitions are reserved before
queue creation; an uncertain create is reconciled, not automatically repeated.
Answers and rendered publication bytes are saved before GitHub publication.
Deterministic feedback event IDs prevent duplicate replanning after a crash.

For the current serial execution policy, pending conversations hold new dispatch
and final readiness while existing workers finish. Collection and publication of
already completed work continue. Conversation tasks use the configured native
planner's exact model and reasoning, with a ten-minute limit and one attempt.
Feedback replanning consumes the existing remediation budget; it cannot reset
that budget. At most 100 conversations can be pending; overflow stays in the
webhook spool for retry rather than being acknowledged and lost.

Changed/deleted comments or revoked actor trust suppress stale replies. A missing
task after uncertain creation, invalid result, or changed plan/head is reported
by the controller for operator attention; it must not silently start another
model or discard an accepted answer. Keep conversation jobs drained before
changing their runtime profile or skill links.

The worker sees bounded excerpts of the issue, recent discussion and latest plan,
plus the source comment and current case binding. It can inspect repository source
read-only and must disclose missing evidence. Read-only is the task's mandate;
this feature does not add a new OS-level sandbox. It inherits the existing native
Hermes execution boundary.

## Enablement checklist

1. Install a tested release containing schema 13 and the `conversation` skill.
   Preserve the ledger and running jobs during the normal paused rollout.
2. Add **Issue comments**, **Pull request reviews**, and **Pull request review
   comments** to the existing repository webhook subscriptions, preserving
   **Issues** and the existing URL/secret. These correspond to `issue_comment`,
   `pull_request_review`, and `pull_request_review_comment` in the
   [GitHub webhook reference](https://docs.github.com/en/webhooks/webhook-events-and-payloads).
   No new reviewer App or account is needed.
3. With native jobs drained, set `conversations_enabled: true` and run
   `bootstrap-runtime` to create the conversation profile before resuming.
   This one operational switch is excluded from immutable case-policy snapshots:
   do not bump the policy revision just to enable it. Retain a copy of the prior
   file for rollback. All actor, model and workflow settings remain immutable;
   changing those still requires a separate policy transition. Never rebind an
   active case or rewrite its accepted policy/history to enable conversations.
4. Test one trusted mention on an unlabelled issue: exactly one readable reply,
   no new case, no builder. Redelivery must not create a second task/reply.
5. Test a substantive reply to a waiting Pip question, then in-scope PR feedback:
   retained evidence, one bounded replanning handoff, ordinary build/CI/reviews.
6. Verify bot/self filtering, edited comments and a restart with an outstanding
   reply. Observe controller `conversation` results alongside normal case reports.

There is deliberately no magic `approve`/`reject` comment grammar. Natural-language
answers are planning evidence, not an approval bypass. Sensitive scope decisions
and human-only merging retain their existing restrictions.
