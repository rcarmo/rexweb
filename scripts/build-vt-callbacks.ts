import wabt from '../web/node_modules/wabt';
const compiler=await wabt();
const file='internal/vt/callbacks.wat';
const parsed=compiler.parseWat(file,await Bun.file(file).text(),{reference_types:true,bulk_memory:true});
try {
 parsed.resolveNames();parsed.validate();
 await Bun.write('internal/vt/callbacks.wasm',parsed.toBinary({write_debug_names:true}).buffer);
 console.log('Built server VT callback trampoline.');
} finally {parsed.destroy();}
