// Types for the shared guide module, so the TypeScript web page can import it.
export type GuideContext = { platform?: 'darwin' | 'win32'; arch?: string; home?: string };
export type Guide = { id: string; title: string; asked: string; why: string; hint: string[]; steps: string[]; sources: string[] };
export const topics: string[];
export function guide(id: string, ctx?: GuideContext): Guide;
export function formatGuide(item: Guide, options?: { columns?: number }): string;
export function guideForValue(name: string): string | null;
export function guideForServer(name: string): string | null;
export function valueProblem(name: string, value: string): string | null;
export function windowsHelperName(arch: string): string;
export function windowsHelperPathProblem(input: string, options?: { arch?: string }): string | undefined;
export function macComputerAppProblem(input: string): string | undefined;
export function unquote(value: string): string;
export function guideList(ctx?: GuideContext): string;
export function guidesMarkdown(): string;
export function printGuide(ui: { log: { message(text: string): void } }, item: Guide, options?: { columns?: number }): void;
