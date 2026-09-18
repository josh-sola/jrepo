import { Button } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
import type { PrFile } from '../../shared/github.ts';

// Shown in the file sidebar slot while the diff is still loading or failed
// to load. The PR metadata already has paths and statuses, so the sidebar
// doesn't have to sit empty.
export function DiffSidebarLoading({ files }: { files: PrFile[] }) {
  return (
    <nav className="sticky top-(--toolbar-height) flex max-h-[calc(100vh-var(--toolbar-height))] flex-col gap-0.5 overflow-y-auto p-2 text-sm">
      {files.map((file) => (
        <div key={file.path} className="flex items-center gap-2 px-2 py-1">
          <Skeleton className="h-1.5 w-1.5 shrink-0 rounded-full" />
          <span className="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground">
            {file.path}
          </span>
        </div>
      ))}
    </nav>
  );
}

export function DiffMainLoading() {
  return (
    <div className="flex min-h-[40vh] items-center justify-center text-sm text-muted-foreground">
      Loading diff…
    </div>
  );
}

export function DiffErrorPanel({
  message,
  onRetry,
}: {
  message: string;
  onRetry: () => void;
}) {
  return (
    <div className="flex min-h-[40vh] items-center justify-center p-6">
      <div className="flex max-w-md flex-col items-center gap-3 text-center">
        <p className="text-sm font-medium text-destructive">
          Could not load the diff.
        </p>
        <p className="font-mono text-xs text-muted-foreground">{message}</p>
        <Button variant="outline" size="sm" onClick={onRetry}>
          Retry
        </Button>
      </div>
    </div>
  );
}
