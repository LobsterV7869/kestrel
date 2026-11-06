export function formatCount(value: number): string {
  if (value >= 1_000_000) {
    return `${(value / 1_000_000).toFixed(value >= 10_000_000 ? 0 : 1)}M`;
  }
  if (value >= 1_000) {
    return `${(value / 1_000).toFixed(value >= 10_000 ? 0 : 1)}K`;
  }
  return String(value);
}

export function formatBytes(value: number): string {
  if (!value) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  let size = value;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit++;
  }
  return `${size.toFixed(size >= 10 || unit === 0 ? 0 : 1)} ${units[unit]}`;
}

export function relativeTime(iso: string): string {
  const time = Date.parse(iso);
  if (Number.isNaN(time)) return "recently";
  const diff = Math.max(0, Date.now() - time);
  const mins = Math.floor(diff / 60_000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d ago`;
  const months = Math.floor(days / 30);
  if (months < 12) return `${months}mo ago`;
  return `${Math.floor(months / 12)}y ago`;
}

export function loaderLabel(loader: string): string {
  switch (loader) {
    case "fabric":
      return "Fabric";
    case "quilt":
      return "Quilt";
    case "neoforge":
      return "NeoForge";
    case "forge":
      return "Forge";
    default:
      return "Vanilla";
  }
}

export function phaseLabel(phase: string | undefined): string {
  switch (phase) {
    case "downloading":
      return "Downloading mods";
    case "verifying":
      return "Verifying files";
    case "committing":
      return "Installing into the instance";
    case "done":
      return "Finished";
    case "failed":
      return "Install failed";
    default:
      return "Preparing";
  }
}

export function titleCase(value: string): string {
  return value
    .split(/[-_\s]+/)
    .filter(Boolean)
    .map((part) => part.slice(0, 1).toUpperCase() + part.slice(1))
    .join(" ");
}
