// Opens one category of the Settings sheet, where agents, skills and providers now live.
import type { Page } from '@playwright/test';

export async function openSettings(page: Page, category: 'Appearance' | 'Providers' | 'Agents' | 'Skills' | 'MCP servers' | 'Schedules' | 'Analytics' | 'Virtualisation') {
  const dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
  // A sheet that is animating closed still exists; only one that is open can be reused.
  const state = await dialog.getAttribute('data-state', { timeout: 250 }).catch(() => null);
  if (state !== 'open') {
    await dialog.waitFor({ state: 'hidden' });
    // Settings sits at the foot of the sidebar; on a phone the sidebar is a drawer.
    const drawer = page.getByRole('button', { name: 'Open navigation', exact: true });
    if (await drawer.isVisible()) await drawer.click();
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
  }
  await dialog.getByRole('tab', { name: category, exact: true }).click();
  return dialog;
}

// Opens the composer's Tools dropdown, where a conversation's capabilities are switched, and
// returns it. Each capability is a switch with its own name.
export async function openTools(page: Page) {
  const tools = page.getByRole('dialog', { name: 'Tools for this conversation' });
  if (!(await tools.isVisible())) await page.getByRole('button', { name: 'Tools', exact: true }).click();
  await tools.waitFor();
  return tools;
}

// The new conversation button lives in the sidebar. On a narrow window the sidebar is a drawer, so
// the drawer is opened first. Returns the button, ready to be pressed or looked at.
export async function newChatButton(page: Page) {
  const drawer = page.getByRole('button', { name: 'Open navigation', exact: true });
  if (await drawer.isVisible()) await drawer.click();
  const button = page.getByRole('button', { name: 'New chat', exact: true }).filter({ visible: true });
  await button.waitFor();
  return button;
}
