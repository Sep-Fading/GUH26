// The words a delegated run is shown with, shared by its block in the conversation and its card in
// the workspace's Agents tab, so both say the same thing.
import type { DelegatedRun } from './chat.svelte';

/** Says where a run is: waiting for the user, running (and how), or how it ended. */
export function runPhase(run: DelegatedRun) {
  return run.status === 'asking' ? 'Waiting for your answer'
    : run.status === 'running' ? (run.mode !== 'background' ? 'Running, the conversation waits'
      // A run the user waited for that was still going at the hour (lead decision, 2026-10-11).
      : run.handedOver ? 'Continues in the background after the 1 h wait' : 'Running in the background')
    : run.status === 'done' ? 'Finished' : run.status === 'declined' ? 'Declined' : 'Did not finish';
}

/** The run's one-line description: the summary its request gave, or for an older run its task's first line. */
export function runSummary(run: DelegatedRun) {
  if (run.summary) return run.summary;
  const first = run.task.split('\n')[0].trim();
  return first.length > 80 ? `${first.slice(0, 79)}…` : first;
}

/** The card's changing line: what the run is doing now, or once it ended, the first line of how. */
export function runLine(run: DelegatedRun) {
  if (run.status === 'asking') return 'Waiting for your answer';
  if (run.status === 'running') return run.live || 'Starting…';
  const first = (run.result ?? '').split('\n').map(line => line.trim()).find(line => line) ?? '';
  return first || runPhase(run);
}

/** Words a time limit in minutes as hours and minutes, such as "1 h 30 min". */
export function duration(minutes: number) {
  const hours = Math.floor(minutes / 60), rest = minutes % 60;
  return hours ? (rest ? `${hours} h ${rest} min` : `${hours} h`) : `${rest} min`;
}
