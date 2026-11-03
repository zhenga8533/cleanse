import { screen, waitFor, within } from "@testing-library/react";

/**
 * The grid named `name`, once its rows are there. React Aria can render a table's or list's
 * rows a render after the grid itself, so a test that reads rows as soon as the grid appears
 * sometimes finds none, as it did on the slower Windows runners.
 */
export async function findGrid(name: string | RegExp): Promise<HTMLElement> {
  const grid = await screen.findByRole("grid", { name });
  await waitFor(() => {
    const rows = within(grid).queryAllByRole("row");
    if (!rows.some((row) => within(row).queryAllByRole("columnheader").length === 0)) {
      throw new Error(`the grid ${String(name)} has no rows yet`);
    }
  });
  return grid;
}
