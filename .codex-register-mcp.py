from pathlib import Path
p = Path('src-tauri/src/commands/mod.rs')
s = p.read_text(encoding='utf-8')
s = s.replace('pub mod java;\n', 'pub mod java;\npub mod mcp;\n')
p.write_text(s, encoding='utf-8', newline='\n')

p = Path('src-tauri/src/lib.rs')
s = p.read_text(encoding='utf-8')
s = s.replace(
    '            commands::window::hide_tray_popup,\n',
    '            commands::window::hide_tray_popup,\n            commands::mcp::get_mcp_server_info,\n',
)
p.write_text(s, encoding='utf-8', newline='\n')
