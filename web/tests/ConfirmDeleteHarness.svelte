<script lang="ts">
  // Supplies the application tooltip context for tests.
  import { Tooltip } from 'bits-ui';
  // Exercises the production deletion dialog directly.
  import ConfirmDelete from '../src/ConfirmDelete.svelte';
  let props: { label: string; actionLabel: string; description: string; option?: { label: string; detail: string };
    action: (ticked: boolean) => Promise<void> } = $props();
</script>
<Tooltip.Provider><ConfirmDelete {...props} /></Tooltip.Provider>
