// Trimming a run of characters from the ends of a string in one pass (issue #1105).
// Studio's own. `/x+$/` and `/^x+|x+$/g` look linear but are quadratic: on a long run
// of `x` that does not reach the end, the engine starts a match at every character of
// the run, scans to the run's end and fails there. CodeQL's js/polynomial-redos flags
// them. These helpers walk the string once from each end instead.

/** `text` without the leading characters `strip` accepts. */
export function trimStartWhere(text: string, strip: (char: string) => boolean): string {
  let start = 0;
  while (start < text.length && strip(text.charAt(start))) start += 1;
  return text.slice(start);
}

/** `text` without the trailing characters `strip` accepts. */
export function trimEndWhere(text: string, strip: (char: string) => boolean): string {
  let end = text.length;
  while (end > 0 && strip(text.charAt(end - 1))) end -= 1;
  return text.slice(0, end);
}

/** `text` without the leading and trailing characters `strip` accepts. */
export function trimWhere(text: string, strip: (char: string) => boolean): string {
  return trimEndWhere(trimStartWhere(text, strip), strip);
}
