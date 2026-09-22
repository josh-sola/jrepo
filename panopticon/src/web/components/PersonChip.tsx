import { Check, X } from 'lucide-react';
import { cn } from '@/lib/utils';
import type { ReviewInboxReviewerState } from '../../shared/reviewInbox.ts';

export interface PersonChipProps {
  login: string;
  avatarUrl: string | null;
  // Present on a reviewer chip to tint it by their review state; absent on
  // an author chip, which stays neutral.
  state?: ReviewInboxReviewerState;
  className?: string;
}

const STATE_STYLES: Record<ReviewInboxReviewerState, string> = {
  APPROVED: 'border-emerald-500/50 text-emerald-700 dark:text-emerald-400',
  CHANGES_REQUESTED: 'border-red-500/50 text-red-700 dark:text-red-400',
};

const STATE_ICON: Record<ReviewInboxReviewerState, typeof Check> = {
  APPROVED: Check,
  CHANGES_REQUESTED: X,
};

// An avatar + login chip, used for both a PR's author (neutral) and a
// reviewer (tinted and marked by their latest review state).
export function PersonChip({
  login,
  avatarUrl,
  state,
  className,
}: PersonChipProps) {
  const StateIcon = state ? STATE_ICON[state] : null;
  return (
    <span
      className={cn(
        'inline-flex shrink-0 items-center gap-1 rounded-full border px-1.5 py-0.5 text-xs text-muted-foreground',
        state && STATE_STYLES[state],
        className,
      )}
    >
      {avatarUrl ? (
        <img src={avatarUrl} alt="" className="h-4 w-4 rounded-full" />
      ) : (
        <span className="h-4 w-4 shrink-0 rounded-full bg-muted" />
      )}
      {login}
      {StateIcon && <StateIcon className="h-3 w-3" />}
    </span>
  );
}
