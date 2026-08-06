export function isUnchangedReload(previous: string | undefined, current: string): boolean {
  return previous !== undefined && previous === current;
}
