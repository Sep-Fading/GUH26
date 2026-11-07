export type UsageWindow = { usedPercent: number | null; resetsAt: number | null; windowDurationMins: number | null };
export type Usage = { status: string; ordinaryUsageAllowed: boolean | null; fetchedAt?: string | null;
  buckets: { limitId: string; limitName?: string; primary: UsageWindow | null; secondary: UsageWindow | null }[] };

/** Returns an unknown reading without inventing quota values. */
export const unavailableUsage = (): Usage => ({ status: 'unavailable', buckets: [], ordinaryUsageAllowed: null });

/** Names each reported window consistently in the footer and Analytics. */
export function usageRows(usage: Usage) {
  return usage.buckets.flatMap(bucket => [bucket.primary, bucket.secondary].flatMap((window, index) => {
    if (!window) return [];
    const minutes = window.windowDurationMins;
    const period = minutes === 10080 ? 'Weekly' : minutes === 300 ? '5-hour'
      : minutes ? `${minutes / 60}-hour` : '';
    const name = ['codex', 'claude'].includes(bucket.limitId) ? '' : bucket.limitName ?? bucket.limitId;
    return [{ ...window, id: `${bucket.limitId}-${index}`, label: [name, period].filter(Boolean).join(' · ') || 'Usage' }];
  }));
}

/** Formats a reported reset in local time, preserving unknown dates. */
export function resetTime(seconds: number | null) {
  return seconds === null ? 'Reset time unavailable' : `Resets ${new Date(seconds * 1000).toLocaleString(undefined,
    { weekday: 'short', month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' })}`;
}
