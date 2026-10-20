# PointSpeak

Ditado por voz 100% local para Windows e Linux. Você segura um atalho, fala, solta, e o texto aparece no
programa em que você está digitando (editor, navegador, terminal, chat). Uma pílula pequena perto do cursor
mostra o que está acontecendo: **Gravando** → **Transcrevendo...** → ✓.

A transcrição usa o Whisper rodando no seu computador, em português, inglês ou com detecção automática.
O que estava no seu clipboard antes do ditado volta para lá depois da colagem.

Versão atual: **1.0.0**.

## Requisitos

- **Windows:** Windows 10 ou 11, 64 bits (x64), com processador com **AVX2** (Intel Haswell de 2013 em diante,
  AMD Excavator/Ryzen). O WebView2 vem com o Windows atualizado; se faltar, o instalador instala.
  Para a aceleração por GPU/NPU (DirectML): placa ou NPU com DirectX 12. NPU Intel (Core Ultra) que o
  DirectML não encontra roda pelo cartão `OpenVINO (Intel)`. Sem nada disso, o ditado roda na CPU.
- **Linux:** x64 com AVX2, sessão **X11** ou **Wayland**, bandeja do sistema (no GNOME, a extensão
  AppIndicator). No Wayland, a colagem precisa do `wtype` ou do `ydotool` (com o serviço `ydotoold` rodando).
  O .deb foi feito para Ubuntu 24.04 / Debian e derivados; o AppImage roda em distribuições com glibc igual ou
  mais nova.
- **Disco:** de 75 MB (modelo `tiny`) a 1,6 GB (modelo `large-v3-turbo`) para os modelos, além do app.
- **Internet:** só para baixar os modelos na primeira vez.

## Instalação

### Windows (.exe)

1. Abra o `PointSpeak_1.0.0_x64-setup.exe`.
2. O instalador não é assinado, então o Windows mostra **"O Windows protegeu o computador"** (SmartScreen).
   Clique em **Mais informações** → **Executar assim mesmo**.
3. Siga o instalador. Ele instala só para o seu usuário (`%LOCALAPPDATA%\PointSpeak`) e não pede administrador.

### Linux (.deb)

```sh
sudo apt install ./PointSpeak_1.0.0_amd64.deb
```

O `apt` instala as dependências e, como recomendados, o `wtype` e o `ydotool` (colagem no Wayland). Para
remover: `sudo apt remove point-speak`.

### Linux (AppImage)

```sh
chmod +x PointSpeak_1.0.0_amd64.AppImage
./PointSpeak_1.0.0_amd64.AppImage
```

## Primeiro uso

1. Abra o PointSpeak. **Nenhuma janela abre**: ele fica no ícone da bandeja do sistema (perto do relógio).
2. Clique no ícone (ou abra o menu dele) → **Configurações…**.
3. Em **Modelos**, baixe um modelo e clique em **Usar este**. O `base` (142 MB) é um bom começo; o `small`
   acerta mais e é mais lento; o `tiny` é o mais rápido. O **Parakeet TDT 0.6B v3** (640 MB) é rápido até na
   CPU (cerca de 0,7 s para 14 s de fala) e detecta o idioma sozinho (25 línguas); o `large-v3-turbo` erra
   menos em nomes e termos raros, mas pede NPU/GPU.
4. Em **Idioma**, escolha Português, Inglês ou Automático. Em **Saída**, escolha se o texto é colado onde está o cursor (padrão) ou só copiado para a área de transferência, para você colar com Ctrl+V (o conteúdo anterior dela é substituído).
5. Feche as Configurações. Quando a bandeja mostrar `Pronto (Alt+Z)`, clique onde quer escrever,
   **segure `Alt+Z`**, fale e solte. O texto é colado ali.

Em **Atalho** você troca a combinação e o modo: **Segurar para falar** (padrão) ou **Tocar para começar e
parar** (Toggle), com a opção de parar sozinho depois de 2,5 s de silêncio. Em **Hardware** você escolhe onde
a transcrição roda (Automático, NPU/GPU via DirectML no Windows, ou CPU). Pelo menu da bandeja dá para
**Pausar ditado** (o atalho volta a ser do app em foco) e **Sair**.

## Privacidade

- O áudio fica só na memória enquanto é transcrito e depois é descartado. Nada é gravado em disco.
- A transcrição roda no seu computador. O texto ditado e o conteúdo do clipboard não vão para a internet
  nem para logs. Não há telemetria nem conta.
- A internet só é usada para baixar os modelos (do Hugging Face, por HTTPS), e cada arquivo é conferido
  pelo SHA-256 antes de ser usado.
- O texto ditado passa pelo clipboard por ~400 ms no Windows (~50 ms no Linux) para ser colado e é marcado
  para não entrar no histórico do clipboard (Win+V no Windows, Klipper e similares no Linux).

## Problemas comuns

- **O app fecha ao transcrever (Windows ou Linux):** o processador não tem AVX2. Esta versão exige AVX2;
  quem compila o projeto pode gerar uma versão para CPUs antigas (veja `docs/tecnologia.md`).
- **"O Windows protegeu o computador" ao instalar:** é o SmartScreen, porque o instalador não é assinado.
  Use **Mais informações** → **Executar assim mesmo**.
- **No Wayland a pílula não segue o cursor:** é esperado. O Wayland não informa a posição do cursor a outros
  apps, então a pílula fica fixa no centro, embaixo da tela.
- **No Wayland o texto não é colado (`Falha ao colar`):** instale o `wtype` (`sudo apt install wtype`) ou o
  `ydotool` com o `ydotoold` ativo.
- **No Wayland (Ubuntu 22.04 ou mais novo, Fedora…) o atalho não faz nada, ou o `Ctrl+Ctrl` no Linux:** o
  PointSpeak lê o teclado direto do sistema (`/dev/input`), o que pede o grupo `input`. Rode
  `sudo usermod -aG input $USER`, saia da sessão e entre de novo. A seção **Atalho** avisa quando falta isso.
  O grupo deixa os apps do seu usuário lerem o teclado; o PointSpeak só reage às teclas do atalho.
- **A aceleração (DirectML) não ligou e está usando a CPU:** o ditado continua funcionando na CPU. O motivo
  aparece em **Configurações → Hardware**, no aviso "A aceleração … não iniciou. O PointSpeak está usando
  a CPU. Motivo: …" (por exemplo, a variante ONNX do modelo ainda não foi baixada: com DirectML escolhido,
  baixe o modelo de novo em **Modelos**). **Tentar de novo** refaz a detecção. Para ver mais detalhes, rode o
  app por um terminal: as mensagens `[pointspeak] …` mostram a engine e o motivo do fallback. No Linux,
  basta rodar `pointspeak` no terminal. No Windows, feche o app pela bandeja e abra pelo PowerShell com
  `Start-Process "$env:LOCALAPPDATA\PointSpeak\pointspeak.exe" -RedirectStandardError "$env:TEMP\pointspeak.log"`;
  as mensagens ficam nesse arquivo (o app ainda não grava log próprio: a pasta de logs das Configurações
  fica vazia).
- **A NPU não aparece em uso no Gerenciador de Tarefas:** com `NPU (DirectML)` (ou `Automático`) escolhido, veja
  o rodapé das Configurações. `Pronto · <modelo> · DirectML · NPU` = a transcrição roda na NPU (o gráfico só sobe
  enquanto transcreve). `· CPU` = a variante ONNX do modelo não foi baixada (em **Modelos**, com o DirectML
  escolhido, o modelo aparece como `Baixar`: baixe-o de novo) ou a aceleração falhou (veja o aviso em
  **Hardware**). `· DirectML · GPU` = a NPU não iniciou, não abriu o modelo ou calculou errado na verificação feita ao
  carregar o modelo (`NPU reprovou na verificação`) e a GPU assumiu; o motivo aparece em **Hardware**.
- **O texto sai sem sentido (palavras de várias línguas, repetidas):** use **Configurações → Microfone →
  Testar transcrição** e veja o dispositivo no diagnóstico. Ao carregar o modelo no DirectML, o PointSpeak
  compara a NPU e a GPU com a CPU e troca de dispositivo se o resultado divergir; se ainda assim o texto
  sair errado, escolha `CPU` em **Hardware**. O cartão `NPU (DirectML)` diz se o Windows encontrou uma NPU (`NPU detectada`).
- **Tenho NPU Intel, mas o cartão `NPU (DirectML)` diz `Sem NPU`:** em alguns Core Ultra o DirectML não enxerga a
  NPU. Escolha `OpenVINO (Intel)` em **Hardware** (ele mostra `NPU detectada · Intel`): o modelo roda na NPU e na
  GPU juntas. **Na primeira vez com cada modelo**, o OpenVINO compila o modelo para a NPU: o app fica em
  `Carregando modelo…` por vários minutos (cerca de 17 min no `large-v3-turbo`). Depois isso fica guardado na pasta
  do modelo (cerca de 1,4 GB no `large-v3-turbo`) e a carga leva segundos.
- **`Microfone indisponível`:** confira o microfone em **Configurações → Microfone** (**Testar microfone**)
  e nas permissões de microfone do sistema.
- **`Nada foi ouvido`:** a gravação ficou sem fala (ou foi curta demais). Fale mais perto do microfone ou
  segure o atalho por mais tempo.
- **O PointSpeak ocupa muita memória parado:** o modelo fica carregado para o ditado começar na hora (de
  ~300 MB no `tiny`/`base` na CPU a ~4 GB no `large-v3-turbo` na NPU/GPU). Em **Hardware → Liberar a memória do
  modelo**, escolha depois de quantos minutos sem uso ele sai da memória; ao apertar o atalho de novo ele
  carrega enquanto você fala (alguns segundos no `large-v3-turbo`).
- **"Ditado indisponível: Pouca memória livre…":** carregar o modelo usa mais memória que mantê-lo pronto (até
  ~8 GB no `large-v3-turbo` no OpenVINO). Para não travar a máquina, o PointSpeak não carrega quando a memória
  livre não dá: feche alguns programas e dite de novo, ou use um modelo menor (`base`, `small`) ou o DirectML.
- **Atalho com dois toques (Alt+Alt):** em **Atalho → Alterar**, toque duas vezes rápido em `Alt`, `Ctrl` ou
  `Shift`. No modo **Segurar para falar**, segure o segundo toque enquanto fala. Só no Windows.
- **O atalho não faz nada:** veja se o ditado está pausado (menu da bandeja) ou se outro app já usa a mesma
  combinação; troque em **Configurações → Atalho**.

## Créditos dos modelos

- **Whisper** (OpenAI, licença MIT): ggml do [whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp) e
  ONNX do [onnx-community](https://huggingface.co/onnx-community).
- **Parakeet TDT 0.6B v3** © NVIDIA, licença [CC-BY-4.0](https://creativecommons.org/licenses/by/4.0/)
  ([modelo original](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3)). O PointSpeak baixa, sem
  modificações, o export ONNX INT8 de [istupakov/parakeet-tdt-0.6b-v3-onnx](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx);
  a decodificação segue a da biblioteca [onnx-asr](https://github.com/istupakov/onnx-asr).

## Documentação

- [`docs/checklist-manual.md`](docs/checklist-manual.md): roteiro de aceite da versão 1.0.0 (Windows, X11 e Wayland).
- [`docs/produto.md`](docs/produto.md): o que é o produto e o plano.
- [`docs/tecnologia.md`](docs/tecnologia.md): stack, como compilar, gerar os instaladores e rodar os testes;
  tamanhos e SHA-256 dos instaladores 1.0.0; segurança e privacidade.
- [`docs/estrutura.md`](docs/estrutura.md): onde fica cada coisa no código.
- [`docs/design/`](docs/design/): o visual do HUD e das Configurações.
- [`AGENTS.md`](AGENTS.md): instruções para quem desenvolve (pessoas e agentes de IA).

---
Projeto criado pela Fábrica de software do runner.
