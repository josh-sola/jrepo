import { useEffect, useRef, useState } from 'react';
import { AlertTriangle, Check, Loader2 } from 'lucide-react';
import { Badge } from '@/components/ui/badge';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import { useHoverStatus } from '../../hooks/useHover.ts';
import type { PrParams } from '../../hooks/usePrData.ts';

const READY_FADE_MS = 4000;

export function HoverStatusBadge({ params }: { params: PrParams }) {
  const { data } = useHoverStatus(params);
  const servers = data ? Object.values(data.servers) : [];
  const starting = servers.some((state) => state === 'starting');
  const failed = servers.some((state) => state === 'failed');
  const isReady = data?.tree === 'ready' && !starting && !failed;

  const [showReady, setShowReady] = useState(false);
  const wasReadyRef = useRef(false);
  useEffect(() => {
    if (isReady && !wasReadyRef.current) {
      wasReadyRef.current = true;
      setShowReady(true);
      const timer = setTimeout(() => setShowReady(false), READY_FADE_MS);
      return () => clearTimeout(timer);
    }
    if (!isReady) wasReadyRef.current = false;
  }, [isReady]);

  if (!data || data.tree === 'none') return null;

  if (data.tree === 'failed') {
    return (
      <Tooltip>
        <TooltipTrigger asChild>
          <Badge variant="destructive" className="gap-1">
            <AlertTriangle className="h-3 w-3" />
            Types failed
          </Badge>
        </TooltipTrigger>
        <TooltipContent>
          {data.treeError ?? 'Could not prepare types for this pull request'}
        </TooltipContent>
      </Tooltip>
    );
  }

  if (data.tree === 'provisioning' || starting) {
    return (
      <Badge variant="secondary" className="gap-1 text-muted-foreground">
        <Loader2 className="h-3 w-3 animate-spin" />
        Preparing types
      </Badge>
    );
  }

  if (showReady) {
    return (
      <Badge
        variant="secondary"
        className="gap-1 text-muted-foreground transition-opacity duration-500"
      >
        <Check className="h-3 w-3" />
        Types ready
      </Badge>
    );
  }

  return null;
}
