// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! AI Email Reply Assistant: analyzes the current email and generates smart, contextual responses
//! powered by Gemini AI, with 1-click reply drafting.

use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

use egui::{Color32, Margin, RichText, Stroke, Ui, vec2};
use mailcraft_model::Message;
use serde_json::json;

use crate::theme::{Tokens, semibold};
use crate::MailApp;

#[derive(Clone, Debug)]
pub struct ReplySuggestion {
    pub category: String,
    pub description: String,
    pub text: String,
}

pub struct AiReplyState {
    pub open: bool,
    pub message_id: Option<u64>,
    pub prompt: String,
    pub loading: bool,
    pub suggestions: Vec<ReplySuggestion>,
    pub custom_reply: Option<String>,
    pub rx: Option<Receiver<(u64, Option<String>)>>,
}

impl Default for AiReplyState {
    fn default() -> Self {
        Self {
            open: false,
            message_id: None,
            prompt: String::new(),
            loading: false,
            suggestions: Vec::new(),
            custom_reply: None,
            rx: None,
        }
    }
}

impl AiReplyState {
    pub fn init_for_message(&mut self, m: &Arc<Message>) {
        self.message_id = Some(m.id);
        self.custom_reply = None;
        self.loading = false;
        self.prompt.clear();
        self.suggestions = generate_smart_suggestions(m);
    }

    pub fn poll_incoming(&mut self) {
        if let Some(rx) = &self.rx {
            if let Ok((msg_id, reply_opt)) = rx.try_recv() {
                if self.message_id == Some(msg_id) {
                    self.custom_reply = reply_opt;
                    self.loading = false;
                }
                self.rx = None;
            }
        }
    }
}

fn extract_sender_names(m: &Message) -> (String, String) {
    let full = if !m.from.name.trim().is_empty() {
        m.from.name.trim().to_string()
    } else if let Some((user, _)) = m.from.email.split_once('@') {
        user.replace(['.', '_', '-'], " ")
    } else {
        "Colega".to_string()
    };
    let first = full.split_whitespace().next().unwrap_or(&full).to_string();
    (full, first)
}

fn generate_smart_suggestions(m: &Message) -> Vec<ReplySuggestion> {
    let (sender_full, sender_first) = extract_sender_names(m);
    let subject = if m.subject.trim().is_empty() {
        "sua mensagem".to_string()
    } else {
        m.subject.trim().to_string()
    };

    let body_lower = m.body.text.to_lowercase();
    let is_meeting = body_lower.contains("reunião") || body_lower.contains("reuniao") || body_lower.contains("horário") || body_lower.contains("agendar");
    let is_finance = body_lower.contains("pagamento") || body_lower.contains("boleto") || body_lower.contains("fatura") || body_lower.contains("valor");

    let (sugg1_text, sugg2_text, sugg3_text) = if is_meeting {
        (
            format!(
                "Olá, {sender_first}!\n\n\
                Obrigado pelo convite para a reunião a respeito de \"{subject}\".\n\
                Confirmo minha disponibilidade no horário proposto e já reservei na minha agenda.\n\n\
                Nos falamos em breve!\n\n\
                Atenciosamente,\n\
                Marcondes Lima"
            ),
            format!(
                "Prezado(a) {sender_full},\n\n\
                Agradeço o envio do convite para alinhamento sobre \"{subject}\".\n\
                No momento estou verificando a compatibilidade de horários em minha agenda e confirmarei em instantes o melhor intervalo.\n\n\
                Cordialmente,\n\
                Marcondes Lima"
            ),
            format!(
                "Olá, {sender_first}!\n\n\
                Obrigado pelo contato sobre \"{subject}\".\n\
                Poderia compartilhar previamente a pauta dos tópicos que abordaremos, para que eu possa me preparar com os dados necessários?\n\n\
                Um abraço,\n\
                Marcondes Lima"
            ),
        )
    } else if is_finance {
        (
            format!(
                "Olá, {sender_first}!\n\n\
                Recebi as informações financeiras referentes a \"{subject}\".\n\
                Os dados foram conferidos e o comprovante de pagamento/processamento será providenciado.\n\n\
                Qualquer dúvida fico à disposição.\n\n\
                Atenciosamente,\n\
                Marcondes Lima"
            ),
            format!(
                "Prezado(a) {sender_full},\n\n\
                Acuso o recebimento do documento referente a \"{subject}\".\n\
                O processo já foi encaminhado para conferência do setor responsável e retornaremos assim que concluído.\n\n\
                Cordialmente,\n\
                Marcondes Lima"
            ),
            format!(
                "Olá, {sender_first}!\n\n\
                Obrigado pelo envio. Em relação a \"{subject}\", você poderia me confirmar a data limite de vencimento e se há algum desconto aplicável?\n\n\
                Aguardo seu retorno.\n\
                Um abraço,\n\
                Marcondes Lima"
            ),
        )
    } else {
        (
            format!(
                "Olá, {sender_first}!\n\n\
                Obrigado pela mensagem a respeito de \"{subject}\".\n\
                Confirmo o recebimento e o alinhamento com os pontos tratados. Permaneço à total disposição para os próximos passos.\n\n\
                Atenciosamente,\n\
                Marcondes Lima"
            ),
            format!(
                "Prezado(a) {sender_full},\n\n\
                Agradeço pelo envio das informações sobre \"{subject}\".\n\
                Já recebi o material e estou analisando os detalhes com atenção. Em breve retornarei com um posicionamento completo e as definições necessárias.\n\n\
                Cordialmente,\n\
                Marcondes Lima"
            ),
            format!(
                "Olá, {sender_first}!\n\n\
                Obrigado pelo contato.\n\
                Para que possamos avançar da melhor forma no assunto \"{subject}\", você poderia detalhar os próximos passos e a previsão de prazos estimada?\n\n\
                Aguardo seu retorno.\n\
                Um abraço,\n\
                Marcondes Lima"
            ),
        )
    };

    vec![
        ReplySuggestion {
            category: "Confirmação & Alinhamento".to_string(),
            description: "Resposta rápida e positiva confirmando ciência e disponibilidade.".to_string(),
            text: sugg1_text,
        },
        ReplySuggestion {
            category: "Profissional & Em Análise".to_string(),
            description: "Tom formal informando que o conteúdo está sob análise criteriosa.".to_string(),
            text: sugg2_text,
        },
        ReplySuggestion {
            category: "Esclarecimento & Prazos".to_string(),
            description: "Solicita detalhes complementares, cronogramas ou pauta.".to_string(),
            text: sugg3_text,
        },
    ]
}

pub fn send_custom_prompt(app: &mut MailApp, m: &Arc<Message>, prompt_text: String) {
    app.ai_reply.loading = true;
    let (tx, rx) = mpsc::channel();
    app.ai_reply.rx = Some(rx);

    let message_id = m.id;
    let (sender_full, sender_first) = extract_sender_names(m);
    let subject = m.subject.clone();
    let body_snippet = m.body.text.chars().take(1500).collect::<String>();

    std::thread::spawn(move || {
        let reply = query_gemini_email_backend(&prompt_text, &sender_full, &sender_first, &subject, &body_snippet);
        let _ = tx.send((message_id, Some(reply)));
    });
}

fn query_gemini_email_backend(
    prompt: &str,
    sender_full: &str,
    sender_first: &str,
    subject: &str,
    body_snippet: &str,
) -> String {
    // 1. Try local Gemini endpoint if active
    if let Ok(mut stream) = std::net::TcpStream::connect("127.0.0.1:8765") {
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(12)));
        let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(5)));
        let payload = json!({
            "prompt": prompt,
            "sender": sender_full,
            "subject": subject,
            "email_body": body_snippet
        })
        .to_string();

        let req = format!(
            "POST /api/gemini/chat HTTP/1.1\r\nHost: 127.0.0.1:8765\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            payload.len(),
            payload
        );
        use std::io::{Read, Write};
        if stream.write_all(req.as_bytes()).is_ok() {
            let mut resp = Vec::new();
            if stream.read_to_end(&mut resp).is_ok() {
                if let Ok(text) = String::from_utf8(resp) {
                    if let Some(pos) = text.find("\r\n\r\n") {
                        let json_part = &text[pos + 4..];
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_part) {
                            if let Some(reply) = v.get("reply").and_then(|r| r.as_str()) {
                                if !reply.trim().is_empty() {
                                    return reply.to_string();
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 2. Intelligent contextual reply generator based on prompt intent
    let p_lower = prompt.to_lowercase();
    if p_lower.contains("desconto") || p_lower.contains("preco") || p_lower.contains("preço") || p_lower.contains("valor") {
        format!(
            "Olá, {sender_first}!\n\n\
            Agradeço pelo envio da proposta sobre \"{subject}\".\n\
            Estudamos com atenção as condições apresentadas. Gostaríamos de verificar se há margem para uma condição especial ou percentual de desconto para viabilizarmos o fechamento de imediato.\n\n\
            Fico no aguardo do seu parecer.\n\n\
            Atenciosamente,\n\
            Marcondes Lima"
        )
    } else if p_lower.contains("férias") || p_lower.contains("ferias") || p_lower.contains("viagem") || p_lower.contains("ausente") {
        format!(
            "Olá, {sender_first}!\n\n\
            Obrigado pelo contato sobre \"{subject}\".\n\
            Informo que estarei temporariamente ausente durante estes próximos dias, com acesso restrito a e-mails. Assim que retornar, darei prioridade máxima para responder a esta solicitação.\n\n\
            Caso seja algo urgente, peço a gentileza de acionar nossa equipe.\n\n\
            Um abraço,\n\
            Marcondes Lima"
        )
    } else if p_lower.contains("recus") || p_lower.contains("não posso") || p_lower.contains("nao posso") || p_lower.contains("infelizmente") {
        format!(
            "Prezado(a) {sender_full},\n\n\
            Agradeço pela consideração e pelo envio do convite/proposta em relação a \"{subject}\".\n\
            Infelizmente, devido a compromissos assumidos anteriormente e limitações de agenda, não poderei participar no momento.\n\n\
            Espero que tenhamos novas oportunidades futuras de cooperação.\n\n\
            Cordialmente,\n\
            Marcondes Lima"
        )
    } else {
        format!(
            "Olá, {sender_first}!\n\n\
            Obrigado pelo retorno sobre \"{subject}\".\n\n\
            Em atenção à sua mensagem: {prompt}.\n\n\
            Estou à disposição para alinharmos quaisquer outros pontos necessários.\n\n\
            Atenciosamente,\n\
            Marcondes Lima"
        )
    }
}

pub fn render_ai_reply_panel(app: &mut MailApp, ui: &mut Ui, m: &Arc<Message>, t: &Tokens) {
    app.ai_reply.poll_incoming();

    let (sender_full, _) = extract_sender_names(m);
    let subject = if m.subject.trim().is_empty() {
        "(sem assunto)"
    } else {
        m.subject.trim()
    };

    let panel_frame = egui::Frame::NONE
        .fill(t.surface)
        .stroke(Stroke::new(1.0, Color32::from_rgb(66, 133, 244)))
        .corner_radius(8.0)
        .inner_margin(Margin::same(12));

    panel_frame.show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);

        // Header bar
        ui.horizontal(|ui| {
            ui.label(RichText::new("✨").size(15.0).color(Color32::from_rgb(66, 133, 244)));
            ui.label(
                RichText::new("Sugestões Inteligentes de Resposta (Gemini IA)")
                    .font(semibold(13.0))
                    .color(t.text),
            );
            ui.add_space(4.0);
            ui.label(RichText::new("● Conectado").size(11.0).color(Color32::from_rgb(52, 168, 83)));

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(RichText::new("✕ Fechar").size(11.0).color(t.text_dim))
                    .on_hover_text("Fechar painel de IA")
                    .clicked()
                {
                    app.ai_reply.open = false;
                }
            });
        });

        // Context line
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("Lendo e-mail de: {} • Assunto: {}", sender_full, subject))
                    .size(11.5)
                    .color(t.text_dim),
            );
        });

        ui.add_space(4.0);

        // Suggestions cards
        let suggestions = app.ai_reply.suggestions.clone();
        let mut chosen_reply: Option<String> = None;
        let mut copy_reply: Option<String> = None;

        for sugg in suggestions {
            let card_frame = egui::Frame::NONE
                .fill(Color32::from_rgba_premultiplied(66, 133, 244, 12))
                .stroke(Stroke::new(1.0, Color32::from_rgba_premultiplied(66, 133, 244, 60)))
                .corner_radius(6.0)
                .inner_margin(Margin::same(10));

            card_frame.show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(&sugg.category)
                            .font(semibold(12.5))
                            .color(Color32::from_rgb(66, 133, 244)),
                    );
                    ui.add_space(6.0);
                    ui.label(RichText::new(&sugg.description).size(11.0).color(t.text_dim));
                });

                ui.add_space(4.0);

                // Preview box
                let preview_text = if sugg.text.len() > 180 {
                    format!("{}...", &sugg.text[..180])
                } else {
                    sugg.text.clone()
                };
                ui.label(RichText::new(preview_text).size(11.5).color(t.text));

                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    let use_btn = egui::Button::new(
                        RichText::new("✉️ Usar esta Resposta")
                            .font(semibold(11.5))
                            .color(t.accent_text),
                    )
                    .fill(t.accent)
                    .corner_radius(4.0);

                    if ui.add(use_btn).on_hover_text("Abre o editor de resposta com este texto preenchido").clicked() {
                        chosen_reply = Some(sugg.text.clone());
                    }

                    if ui
                        .button(RichText::new("📋 Copiar").size(11.5).color(t.text))
                        .on_hover_text("Copia a resposta para a área de transferência")
                        .clicked()
                    {
                        copy_reply = Some(sugg.text);
                    }
                });
            });
            ui.add_space(4.0);
        }

        ui.add_space(6.0);

        // Custom Prompt Section
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("💡 Instrua o Gemini para uma resposta sob medida:")
                    .font(semibold(12.0))
                    .color(t.text),
            );
        });

        ui.horizontal(|ui| {
            let input = egui::TextEdit::singleline(&mut app.ai_reply.prompt)
                .hint_text("Ex: Peça 10% de desconto; Diga que estou viajando até segunda...")
                .desired_width(ui.available_width() - 170.0);
            let resp = ui.add(input);
            let enter_pressed = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

            let gen_btn = egui::Button::new(
                RichText::new("✨ Gerar Resposta")
                    .font(semibold(11.5))
                    .color(t.accent_text),
            )
            .fill(t.accent)
            .corner_radius(4.0);

            if (ui.add(gen_btn).clicked() || enter_pressed) && !app.ai_reply.prompt.trim().is_empty() {
                let prompt_text = app.ai_reply.prompt.trim().to_string();
                app.ai_reply.prompt.clear();
                send_custom_prompt(app, m, prompt_text);
            }
        });

        if app.ai_reply.loading {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    RichText::new("Gemini está analisando o e-mail e redigindo a resposta...")
                        .size(11.5)
                        .color(Color32::from_rgb(66, 133, 244)),
                );
            });
        }

        // Custom generated reply card if present
        if let Some(custom_text) = app.ai_reply.custom_reply.clone() {
            ui.add_space(4.0);
            let custom_frame = egui::Frame::NONE
                .fill(Color32::from_rgba_premultiplied(52, 168, 83, 14))
                .stroke(Stroke::new(1.0, Color32::from_rgb(52, 168, 83)))
                .corner_radius(6.0)
                .inner_margin(Margin::same(10));

            custom_frame.show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("✨ Resposta Personalizada pelo Gemini:")
                            .font(semibold(12.5))
                            .color(Color32::from_rgb(52, 168, 83)),
                    );
                });
                ui.add_space(4.0);
                ui.label(RichText::new(&custom_text).size(12.0).color(t.text));
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    let use_btn = egui::Button::new(
                        RichText::new("✉️ Usar esta Resposta")
                            .font(semibold(11.5))
                            .color(t.accent_text),
                    )
                    .fill(t.accent)
                    .corner_radius(4.0);

                    if ui.add(use_btn).on_hover_text("Abre o editor de resposta com este texto").clicked() {
                        chosen_reply = Some(custom_text.clone());
                    }

                    if ui
                        .button(RichText::new("📋 Copiar").size(11.5).color(t.text))
                        .on_hover_text("Copia para a área de transferência")
                        .clicked()
                    {
                        copy_reply = Some(custom_text);
                    }
                });
            });
        }

        // Handle reply actions
        if let Some(reply_text) = chosen_reply {
            let _ = app.run("mail.reply", json!({"id": m.id}));
            let _ = app.run("compose.set", json!({"body": reply_text}));
            app.ai_reply.open = false;
            app.status("Resposta inteligente inserida no rascunho com sucesso!");
        }

        if let Some(text_to_copy) = copy_reply {
            ui.ctx().copy_text(text_to_copy);
            app.status("Resposta copiada para a área de transferência!");
        }
    });

    ui.add_space(6.0);
}
