import { cn } from "@/lib/utils";

interface DropOverlayProps {
  active: boolean;
}

export function DropOverlay({ active }: DropOverlayProps) {
  return (
    <div
      className={cn(
        "fixed inset-0 z-[1000] items-center justify-center border-3 border-dashed border-blue-400 bg-zinc-900/85",
        active ? "flex" : "hidden",
      )}
    >
      <span className="font-mono text-xl font-semibold tracking-wide text-blue-400">
        Drop file to upload
      </span>
    </div>
  );
}
