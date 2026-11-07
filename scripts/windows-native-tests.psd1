# The test lists of native Windows acceptance: data only, read with Import-PowerShellDataFile by
# windows-native-acceptance.ps1 and windows-native-standard-user.ps1. Each script holds this
# file's SHA-256 (line endings removed) and refuses a list it was not reviewed with; update both
# constants when a list changes (tests/setup-windows-network.test.mjs checks them).
@{
    # Elevated owner: whole integration targets.
    Integration = @('platform_fs', 'platform_fs_mutation', 'secrets', 'windows_permissions', 'windows_worker', 'agents',
        'windows_startup', 'windows_file_policy', 'windows_broker', 'windows_ports', 'private_setup_file', 'windows_git')
    # Elevated owner: ignored tests, as "target filter".
    Ignored = @(
        'windows_broker sandbox_handles_workspace_above_previous_limit',
        'windows_acl_inheritance blocked_subtree_is_unchanged', 'windows_acl_inheritance new_descendants_are_revoked',
        'windows_acl_inheritance inherited_orphan_recovers', 'windows_acl_inheritance sandbox_links_do_not_block_cleanup',
        'windows_acl_inheritance backend_exit_recovers_without_restart', 'windows_acl_inheritance failed_cleanup_retries_without_restart',
        'windows_acl_inheritance large_tree_is_revoked', 'windows_acl_inheritance internal_hard_links_are_granted',
        'windows_acl_inheritance outside_hard_link_is_refused', 'windows_acl_inheritance protected_hard_link_is_refused',
        'windows_acl_inheritance file_made_by_another_run_is_revoked',
        'windows_git_commands git_arguments_and_boundary',
        # The bundled Git over HTTPS through the approved host's proxy; needs public internet.
        'windows_tools git_ls_remote_through_the_approved_proxy',
        # npm and npx with MiniClaw's npm; the last two need public internet (registry.npmjs.org).
        'windows_npm private_npm_is_verified_and_a_changed_one_is_refused', 'windows_npm npx_arguments_reach_the_server_unchanged',
        'windows_npm shell_npm_and_npx_work_in_a_workspace', 'windows_npm npx_server_installs_from_the_registry_and_starts_offline_afterwards')
    # Elevated owner: unit-test filters of the library.
    Library = @('windows_gate::', 'executor::gate_tests', 'seatbelt::tests', 'seatbelt::windows_root_tests', 'computer::',
        'briefing::tests', 'skills::tests', 'powershell::tests', 'windows_process::tests', 'windows_shell::', 'windows_conpty::',
        'windows_sandbox::native::tests', 'windows_sandbox::native::journal::tests', 'windows_sandbox::cleanup::',
        'scoped::platform_tests', 'windows_network_broker::', 'terminal::windows_tests', 'mcp_import::platform_tests',
        'mcp_servers::runtime_path_tests', 'mcp_client::tests', 'mcp_client::windows_tests')
    # Elevated owner: ignored unit tests of the library.
    LibraryIgnored = @('journal::tests::retired_sid_cannot_authorize_new_sandbox', 'journal::tests::live_member_blocks_absent_job_proof')
    # Elevated owner: ignored tests of windows_runtimes, as "VARIABLE test": the runner's input the test
    # needs, or - for none. A test whose input is not set is named in a warning and not run.
    Runtimes = @('- private_node_is_verified_and_a_changed_one_is_refused', '- shell_node_runs_offline_with_the_private_node',
        # Needs public internet, like windows_tools.
        '- shell_node_https_through_the_proxy',
        'MINICLAW_TEST_MCP_PROJECT sdk_server_runs_confined_with_the_private_node',
        'MINICLAW_TEST_MCP_PROJECT sdk_server_fetches_only_its_approved_host',
        'MINICLAW_TEST_PYTHON python_starts_after_activation_priming', 'MINICLAW_TEST_PY_VENV python_venv_runs_offline_without_asyncio',
        'MINICLAW_TEST_PYTHON python_extension_modules_load_after_priming', '- python_hook_is_verified_and_a_changed_one_is_refused',
        'MINICLAW_TEST_PY_MCP python_mcp_server_runs_without_approved_hosts',
        # Need public internet, like windows_tools.
        'MINICLAW_TEST_PY_MCP python_mcp_server_fetches_only_its_approved_host',
        'MINICLAW_TEST_PYTHON python_asyncio_script_uses_subprocess_and_the_proxy')
    # The gate with this installation's service running, then switched off for one whole process.
    GateReady = 'windows_gate::native::tests::the_gate_follows_the_service'
    GateSwitchedOff = 'executor::gate_tests::the_switch_closes_every_sandboxed_entry_point'
    # Standard user: unit-test filters of the library.
    StandardLibrary = @('windows_gate::', 'executor::gate_tests', 'windows_sandbox::native::tests',
        'windows_sandbox::native::journal::tests', 'windows_network_broker::native::tests', 'windows_process::tests',
        'terminal::windows_tests', 'mcp_client::windows_tests', 'mcp_client::tests', 'mcp_servers::runtime_path_tests', 'powershell::tests')
    # Standard user: exact tests as "binary mode test"; mode "ignored" runs an ignored test.
    StandardExact = @(
        'broker run broker_nested_policy', 'broker run pinned_ancestor', 'broker run shell_appcontainer_boundary',
        'broker run appcontainer_cannot_read_host_credential',
        'native ignored windows_sandbox::native::journal::tests::retired_sid_cannot_authorize_new_sandbox',
        'native ignored windows_sandbox::native::journal::tests::live_member_blocks_absent_job_proof',
        'acl ignored sandbox_links_do_not_block_cleanup', 'acl ignored backend_exit_recovers_without_restart',
        'acl ignored failed_cleanup_retries_without_restart', 'acl ignored internal_hard_links_are_granted',
        'acl ignored outside_hard_link_is_refused', 'acl ignored protected_hard_link_is_refused',
        'acl ignored file_made_by_another_run_is_revoked',
        'network ignored offline_run_stays_closed', 'network ignored exact_proxy_only',
        'network ignored networked_run_stays_closed_elsewhere',
        'runtimes ignored private_node_is_verified_and_a_changed_one_is_refused',
        'runtimes ignored shell_node_runs_offline_with_the_private_node', 'runtimes ignored shell_node_https_through_the_proxy',
        'runtimes ignored python_hook_is_verified_and_a_changed_one_is_refused')
}
