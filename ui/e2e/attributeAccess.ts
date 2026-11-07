/**
 * The gateway's `access/attributes` answer in a stub (API/02 §7b, T-5528): the decisions per
 * attribute the Portal renders, never computes. `write` lists the attributes the person may change.
 */
export function attributeTable(type: string, names: string[], write: string[] = [], remove: string[] = []) {
  return {
    type,
    attributes: names.map((name) => ({
      name,
      read: true,
      write: write.includes(name),
      delete: remove.includes(name),
      reason: write.includes(name) ? "grant" : "not_in_grant",
    })),
  };
}
