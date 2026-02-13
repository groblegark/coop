import { useCallback } from "react";
import { Terminal as XTerm } from "@xterm/xterm";
import { Terminal } from "@/components/Terminal";
import { THEME } from "@/lib/constants";

interface TerminalPreviewProps {
  instance: XTerm;
  /** Cached screen lines to replay after mount. */
  lastScreenLines: string[] | null;
  sourceCols: number;
}

/** Read-only, non-interactive terminal preview anchored to the bottom. */
export function TerminalPreview({ instance, lastScreenLines, sourceCols }: TerminalPreviewProps) {
  const handleReady = useCallback(() => {
    // Re-render cached screen after open() to handle screen_batch that
    // arrived before the terminal was mounted into the DOM.
    if (lastScreenLines) {
      instance.resize(sourceCols, lastScreenLines.length);
      instance.reset();
      instance.write(lastScreenLines.join("\r\n"));
    }
  }, [instance, lastScreenLines, sourceCols]);

  return (
    <div className="pointer-events-none relative flex-1 overflow-hidden">
      <Terminal
        instance={instance}
        theme={THEME}
        className="absolute bottom-0 left-0"
        onReady={handleReady}
      />
    </div>
  );
}
