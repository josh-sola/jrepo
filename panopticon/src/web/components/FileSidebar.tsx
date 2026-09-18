import { Check, Folder } from 'lucide-react';
import { cn } from '@/lib/utils';
import { Badge } from '@/components/ui/badge';
import type { DiffPayload } from '../../shared/diff.ts';

export interface FileSidebarProps {
  files: DiffPayload[];
  isViewed: (path: string) => boolean;
  isCollapsed: (path: string) => boolean;
  activePath: string | null;
  onSelect: (path: string) => void;
}

const STATUS_DOT: Record<DiffPayload['status'], string> = {
  created: 'bg-emerald-500',
  deleted: 'bg-red-500',
  changed: 'bg-amber-500',
};

function dirOf(path: string): string {
  const idx = path.lastIndexOf('/');
  return idx === -1 ? '' : path.slice(0, idx);
}

function baseNameOf(path: string): string {
  const idx = path.lastIndexOf('/');
  return idx === -1 ? path : path.slice(idx + 1);
}

function groupByDirectory(files: DiffPayload[]): [string, DiffPayload[]][] {
  const groups = new Map<string, DiffPayload[]>();
  for (const file of files) {
    const dir = dirOf(file.path);
    const group = groups.get(dir) ?? [];
    group.push(file);
    groups.set(dir, group);
  }
  return [...groups.entries()].sort(([a], [b]) => a.localeCompare(b));
}

function FileRow({
  file,
  active,
  viewed,
  onSelect,
}: {
  file: DiffPayload;
  active: boolean;
  viewed: boolean;
  onSelect: (path: string) => void;
}) {
  return (
    <button
      type="button"
      onClick={() => onSelect(file.path)}
      title={file.oldPath ? `${file.oldPath} → ${file.path}` : file.path}
      className={cn(
        'flex w-full items-center gap-2 rounded-sm px-2 py-1 text-left text-xs hover:bg-accent',
        active && 'bg-accent',
      )}
    >
      <span
        className={cn(
          'h-1.5 w-1.5 shrink-0 rounded-full',
          STATUS_DOT[file.status],
        )}
      />
      <span className="min-w-0 flex-1 truncate font-mono">
        {baseNameOf(file.path)}
      </span>
      <span className="shrink-0 font-mono text-[10px] text-muted-foreground">
        <span className="text-emerald-600 dark:text-emerald-400">
          +{file.stat.added}
        </span>{' '}
        <span className="text-red-600 dark:text-red-400">
          -{file.stat.removed}
        </span>
      </span>
      {viewed && <Check className="h-3 w-3 shrink-0 text-muted-foreground" />}
    </button>
  );
}

export function FileSidebar({
  files,
  isViewed,
  isCollapsed,
  activePath,
  onSelect,
}: FileSidebarProps) {
  const ruleCollapsed = files.filter((f) => f.collapseReason != null);
  const rest = files.filter((f) => f.collapseReason == null);
  const groups = groupByDirectory(rest);

  return (
    <nav className="sticky top-(--toolbar-height) flex max-h-[calc(100vh-var(--toolbar-height))] flex-col gap-3 overflow-y-auto p-2 text-sm">
      {groups.map(([dir, groupFiles]) => (
        <div key={dir || '.'}>
          <div className="flex items-center gap-1 px-2 py-1 text-[11px] font-medium text-muted-foreground">
            <Folder className="h-3 w-3" />
            <span className="truncate">{dir || '(root)'}</span>
          </div>
          <div className="flex flex-col gap-0.5">
            {groupFiles.map((file) => (
              <FileRow
                key={file.path}
                file={file}
                active={activePath === file.path}
                viewed={isViewed(file.path)}
                onSelect={onSelect}
              />
            ))}
          </div>
        </div>
      ))}
      {ruleCollapsed.length > 0 && (
        <div>
          <div className="flex items-center gap-1 px-2 py-1 text-[11px] font-medium text-muted-foreground">
            <span>Collapsed</span>
            <Badge variant="secondary" className="h-4 px-1 text-[10px]">
              {ruleCollapsed.length}
            </Badge>
          </div>
          <div className="flex flex-col gap-0.5">
            {ruleCollapsed.map((file) => (
              <FileRow
                key={file.path}
                file={file}
                active={activePath === file.path}
                viewed={isViewed(file.path)}
                onSelect={onSelect}
              />
            ))}
          </div>
        </div>
      )}
      {files.every((file) => isCollapsed(file.path)) && files.length > 0 && (
        <p className="px-2 text-xs text-muted-foreground">
          Every file is collapsed.
        </p>
      )}
    </nav>
  );
}
