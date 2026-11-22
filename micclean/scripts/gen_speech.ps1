param([string]$Out = "$PSScriptRoot\..\testdata")
Add-Type -AssemblyName System.Speech
New-Item -ItemType Directory -Force $Out | Out-Null
$fmt = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(48000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono)

$texts = @{
  "far_pt" = @("Microsoft Maria Desktop", -1, "Bom dia a todos. Hoje vamos falar sobre o processamento de audio em tempo real. O cancelamento de eco remove o som que sai das caixas e volta pelo microfone. Isso acontece muito quando a pessoa nao usa fone de ouvido. A supressao de ruido remove o ventilador, o teclado e as vozes ao fundo. Depois disso, o compressor e o equalizador deixam a voz mais agradavel. O resultado vai para o cabo virtual, que o OBS e o Discord conseguem ler. Vamos testar tudo com sinais gerados pelo computador e medir os numeros com cuidado. Nada de parece funcionar, queremos resultados objetivos e repetiveis.");
  "near_en" = @("Microsoft Zira Desktop", 0, "Hello, this is the local talker speaking into the microphone. I am talking at the same time as the far end, which is called double talk. The echo canceller must keep my voice intact while it removes the echo coming from the loudspeakers. My words should not be cut or attenuated for a long time. Let us see whether the adaptive filter keeps converging while both people speak together.");
  "babble1" = @("Microsoft Zira Desktop", 2, "In the background somebody is talking about the weather and the traffic downtown. The meeting was moved to Thursday afternoon, and nobody remembered to book the room. Maybe we should order some pizza for everyone.");
  "babble2" = @("Microsoft Maria Desktop", 3, "Ali no fundo alguem comenta sobre o jogo de ontem e o preco do cafe. A reuniao foi adiada para quinta feira e ninguem reservou a sala. Talvez seja melhor pedir uma pizza para todo mundo.");
  "clean_pt" = @("Microsoft Maria Desktop", 0, "Esta e uma gravacao limpa usada para medir a supressao de ruido. A voz precisa continuar clara e natural depois do filtro. O ruido do ventilador e do teclado deve sumir quase por completo, enquanto as palavras continuam inteligiveis. Vamos repetir a medida com varios niveis de ruido para ter certeza.");
}
foreach ($k in $texts.Keys) {
  $v = $texts[$k]
  $s = New-Object System.Speech.Synthesis.SpeechSynthesizer
  $s.SelectVoice($v[0]); $s.Rate = $v[1]
  $s.SetOutputToWaveFile("$Out\$k.wav", $fmt)
  $s.Speak($v[2]); $s.Dispose()
  "{0}: {1:N1} s" -f $k, ((Get-Item "$Out\$k.wav").Length / 96000)
}
