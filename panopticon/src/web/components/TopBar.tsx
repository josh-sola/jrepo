import { useEffect, useState } from 'react';
import { Columns2, Moon, Rows3, Search, Sun } from 'lucide-react';
import { Button } from '@/components/ui/button';
import type { PrParams } from '../hooks/usePrData.ts';
import { HoverStatusBadge } from './hover/HoverStatusBadge.tsx';
import { Toggle } from '@/components/ui/toggle';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from '@/components/ui/command';
import { useTheme } from '../theme.tsx';
import type { ViewMode } from '../hooks/usePrefs.ts';
import type { DiffPayload } from '../../shared/diff.ts';

export interface TopBarProps {
  viewMode: ViewMode;
  onViewModeChange: (mode: ViewMode) => void;
  whitespaceIgnored: boolean;
  onWhitespaceChange: (ignored: boolean) => void;
  files: DiffPayload[];
  onJumpToFile: (path: string) => void;
  params: PrParams;
}

export function TopBar({
  viewMode,
  onViewModeChange,
  whitespaceIgnored,
  onWhitespaceChange,
  files,
  onJumpToFile,
  params,
}: TopBarProps) {
  const { theme, setTheme } = useTheme();
  const [paletteOpen, setPaletteOpen] = useState(false);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if ((event.metaKey || event.ctrlKey) && event.key === 'k') {
        event.preventDefault();
        setPaletteOpen((open) => !open);
      }
    }
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, []);

  return (
    <div className="flex items-center gap-2 border-b border-border bg-card px-3 py-1.5">
      <HoverStatusBadge params={params} />
      <div className="flex items-center overflow-hidden rounded-md border border-border">
        <Toggle
          pressed={viewMode === 'split'}
          onPressedChange={() => onViewModeChange('split')}
          size="sm"
          className="rounded-none gap-1.5"
          aria-label="Split view"
        >
          <Columns2 className="h-4 w-4" />
          Split
        </Toggle>
        <Toggle
          pressed={viewMode === 'unified'}
          onPressedChange={() => onViewModeChange('unified')}
          size="sm"
          className="rounded-none gap-1.5"
          aria-label="Unified view"
        >
          <Rows3 className="h-4 w-4" />
          Unified
        </Toggle>
      </div>
      <Tooltip>
        <TooltipTrigger asChild>
          <Toggle
            pressed={whitespaceIgnored}
            onPressedChange={onWhitespaceChange}
            size="sm"
            aria-label="Ignore whitespace"
          >
            Ignore whitespace
          </Toggle>
        </TooltipTrigger>
        <TooltipContent>
          Only affects files rendered with the plain-text fallback.
        </TooltipContent>
      </Tooltip>
      <div className="flex-1" />
      <Button
        variant="outline"
        size="sm"
        onClick={() => setPaletteOpen(true)}
        className="gap-1.5"
      >
        <Search className="h-4 w-4" />
        Jump to file
        <kbd className="ml-1 rounded border border-border px-1 text-[10px] text-muted-foreground">
          ⌘K
        </kbd>
      </Button>
      <Button
        variant="outline"
        size="icon-sm"
        aria-label="Toggle theme"
        onClick={() => setTheme(theme === 'dark' ? 'light' : 'dark')}
      >
        {theme === 'dark' ? (
          <Sun className="h-4 w-4" />
        ) : (
          <Moon className="h-4 w-4" />
        )}
      </Button>
      <CommandDialog open={paletteOpen} onOpenChange={setPaletteOpen}>
        <CommandInput placeholder="Jump to a file..." />
        <CommandList>
          <CommandEmpty>No matching file.</CommandEmpty>
          <CommandGroup heading="Files">
            {files.map((file) => (
              <CommandItem
                key={file.path}
                value={file.path}
                onSelect={() => {
                  onJumpToFile(file.path);
                  setPaletteOpen(false);
                }}
              >
                {file.path}
              </CommandItem>
            ))}
          </CommandGroup>
        </CommandList>
      </CommandDialog>
    </div>
  );
}
