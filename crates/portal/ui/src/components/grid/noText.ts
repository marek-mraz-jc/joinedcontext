/**
 * The `text` of an actions column. The SDK grid never reads it: an actions column draws its `cell`,
 * search and the filters skip it, and its header offers no sort. `DataColumn` still requires one.
 */
export const noText = (): string => "";
