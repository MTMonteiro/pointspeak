# Fechar as Configurações só esconde: o que grava precisa parar no CloseRequested

tipo: armadilha
origem: task #828 · claude · 2026-10-18

- O `on_window_event` do `src-tauri/src/lib.rs` troca o fechar da janela `settings` por `hide()`. O webview
  continua vivo: os componentes React **não** desmontam e o `cleanup` do `useEffect` não roda.
- Então "limpar ao fechar a janela" tem de vir do backend: o teste de transcrição para no `CloseRequested`
  (`TranscriptionTest::stop_for_window_close`, motivo `windowClosed`) para a tela devolver o atalho
  suspenso. Fechar o microfone junta threads do áudio: faça isso fora da thread principal.
- As seções usam o atributo `hidden`, também sem desmontar: ao esconder uma seção, mude o **estado** do
  componente (não só um ref), senão o botão volta preso no estado antigo.
