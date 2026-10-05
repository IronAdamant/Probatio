const TITLE = /^(?:test|it)\((["'`])(.*?)\1/

export function titlesInText(text: string): string[] {
  const titles: string[] = []
  for (const line of text.split("\n")) {
    const match = line.match(TITLE)
    if (match) titles.push(match[2])
  }
  return titles
}

export function duplicateTitles(files: Array<{ file: string; text: string }>): Array<{ title: string; files: string[] }> {
  const seen = new Map<string, string[]>()
  for (const file of files) {
    for (const title of titlesInText(file.text)) {
      const list = seen.get(title) ?? []
      list.push(file.file)
      seen.set(title, list)
    }
  }
  return [...seen.entries()]
    .filter(([, files]) => files.length > 1)
    .map(([title, files]) => ({ title, files }))
}
