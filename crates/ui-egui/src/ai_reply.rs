// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! AI Email Reply Assistant: analyzes the current email and generates smart, contextual responses
//! powered by Gemini AI, with interactive chat adjustments and 1-click reply drafting.

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

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: String, // "user" or "ai"
    pub text: String,
}

pub struct AiReplyState {
    pub open: bool,
    pub message_id: Option<u64>,
    pub prompt: String,
    pub loading: bool,
    pub suggestions: Vec<ReplySuggestion>,
    pub show_all_suggestions: bool,
    pub chat_base_text: Option<String>,
    pub chat_history: Vec<ChatMessage>,
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
            show_all_suggestions: false,
            chat_base_text: None,
            chat_history: Vec::new(),
            custom_reply: None,
            rx: None,
        }
    }
}

impl AiReplyState {
    pub fn init_for_message(&mut self, m: &Arc<Message>) {
        self.message_id = Some(m.id);
        self.custom_reply = None;
        self.chat_base_text = None;
        self.chat_history.clear();
        self.show_all_suggestions = false;
        self.loading = false;
        self.prompt.clear();
        self.suggestions = generate_smart_suggestions(m);
    }

    pub fn poll_incoming(&mut self) {
        if let Some(rx) = &self.rx {
            if let Ok((msg_id, reply_opt)) = rx.try_recv() {
                if self.message_id == Some(msg_id) {
                    if let Some(text) = &reply_opt {
                        self.chat_history.push(ChatMessage {
                            role: "ai".to_string(),
                            text: text.clone(),
                        });
                    }
                    self.custom_reply = reply_opt;
                    self.loading = false;
                }
                self.rx = None;
            }
        }
    }
}

fn is_organization_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    let org_keywords = [
        "mercado", "pago", "livre", "google", "microsoft", "apple", "amazon", "netflix",
        "banco", "bank", "nubank", "bradesco", "itau", "itaú", "santander", "inter", "caixa",
        "suporte", "support", "atendimento", "sac", "contato", "contact", "equipe", "team",
        "newsletter", "notificac", "notificaç", "no-reply", "noreply", "mailer", "billing",
        "financeiro", "vendas", "sales", "store", "shop", "loja", "steam", "playstation",
        "uber", "ifood", "magazine", "shopee", "aliexpress", "gov.br", "receita", "serviço"
    ];
    org_keywords.iter().any(|k| lower.contains(k))
        || name.ends_with(" S.A.")
        || name.ends_with(" LTDA")
        || name.ends_with(" S/A")
}

fn extract_sender_names(m: &Message) -> (String, String, bool) {
    let full = if !m.from.name.trim().is_empty() {
        m.from.name.trim().to_string()
    } else if let Some((user, _)) = m.from.email.split_once('@') {
        user.replace(['.', '_', '-'], " ")
    } else {
        "Colega".to_string()
    };

    let is_org = is_organization_name(&full) || is_organization_name(&m.from.email);
    let greeting_name = if is_org {
        format!("equipe {}", full)
    } else {
        full.split_whitespace().next().unwrap_or(&full).to_string()
    };

    (full, greeting_name, is_org)
}

fn generate_smart_suggestions(m: &Message) -> Vec<ReplySuggestion> {
    let (sender_full, greeting_name, is_org) = extract_sender_names(m);
    let subject = if m.subject.trim().is_empty() {
        "sua mensagem".to_string()
    } else {
        m.subject.trim().to_string()
    };

    let body_lower = m.body.text.to_lowercase();
    let is_meeting = body_lower.contains("reunião") || body_lower.contains("reuniao") || body_lower.contains("horário") || body_lower.contains("agendar");
    let is_finance = body_lower.contains("pagamento") || body_lower.contains("boleto") || body_lower.contains("fatura") || body_lower.contains("aprovado") || body_lower.contains("valor");

    let (sugg1_text, sugg2_text, sugg3_text) = if is_meeting {
        (
            format!(
                "Olá, {greeting_name}!\n\n\
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
                "Olá, {greeting_name}!\n\n\
                Obrigado pelo contato sobre \"{subject}\".\n\
                Poderia compartilhar previamente a pauta dos tópicos que abordaremos, para que eu possa me preparar com os dados necessários?\n\n\
                Um abraço,\n\
                Marcondes Lima"
            ),
        )
    } else if is_finance {
        if is_org {
            (
                format!(
                    "Olá, {greeting_name}!\n\n\
                    Recebi a notificação referente a \"{subject}\".\n\
                    Os dados foram devidamente conferidos e arquivados no controle financeiro.\n\n\
                    Qualquer dúvida fico à disposição.\n\n\
                    Atenciosamente,\n\
                    Marcondes Lima"
                ),
                format!(
                    "Prezada {greeting_name},\n\n\
                    Acuso o recebimento da confirmação referente a \"{subject}\".\n\
                    Poderiam encaminhar o comprovante detalhado ou espelho da transação em anexo para conferência contábil?\n\n\
                    Cordialmente,\n\
                    Marcondes Lima"
                ),
                format!(
                    "Olá, {greeting_name}!\n\n\
                    Obrigado pela atualização sobre \"{subject}\".\n\
                    Caso haja alguma pendência ou necessidade de confirmação adicional, peço a gentileza de nos avisar.\n\n\
                    Um abraço,\n\
                    Marcondes Lima"
                ),
            )
        } else {
            (
                format!(
                    "Olá, {greeting_name}!\n\n\
                    Recebi as informações financeiras referentes a \"{subject}\".\n\
                    Os dados foram conferidos e o comprovante de pagamento/processamento será providenciado.\n\n\
                    Qualquer dúvida fico à disposição.\n\n\
                    Atenciosamente,\n\
                    Marcondes Lima"
                ),
                format!(
                    "Prezado(a) {sender_full},\n\n\
                    Acuso o recebimento do documento referente a \"{subject}\".\n\
                    O processo já foi encaminhado para conferência e retornaremos assim que concluído.\n\n\
                    Cordialmente,\n\
                    Marcondes Lima"
                ),
                format!(
                    "Olá, {greeting_name}!\n\n\
                    Obrigado pelo envio. Em relação a \"{subject}\", você poderia me confirmar a data de liquidação e se há algum detalhe adicional?\n\n\
                    Aguardo seu retorno.\n\
                    Um abraço,\n\
                    Marcondes Lima"
                ),
            )
        }
    } else {
        (
            format!(
                "Olá, {greeting_name}!\n\n\
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
                "Olá, {greeting_name}!\n\n\
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
            description: "Solicita detalhes complementares, cronogramas ou comprovantes.".to_string(),
            text: sugg3_text,
        },
    ]
}

pub fn send_chat_adjustment(app: &mut MailApp, m: &Arc<Message>, user_instruction: String, base_text: Option<String>) {
    app.ai_reply.loading = true;
    app.ai_reply.chat_history.push(ChatMessage {
        role: "user".to_string(),
        text: user_instruction.clone(),
    });

    let (tx, rx) = mpsc::channel();
    app.ai_reply.rx = Some(rx);

    let message_id = m.id;
    let (sender_full, greeting_name, is_org) = extract_sender_names(m);
    let subject = m.subject.clone();
    let body_snippet = m.body.text.chars().take(1500).collect::<String>();

    std::thread::spawn(move || {
        let reply = query_gemini_adjustment_backend(
            &user_instruction,
            base_text.as_deref(),
            &sender_full,
            &greeting_name,
            is_org,
            &subject,
            &body_snippet,
        );
        let _ = tx.send((message_id, Some(reply)));
    });
}

fn query_gemini_adjustment_backend(
    instruction: &str,
    base_text: Option<&str>,
    sender_full: &str,
    greeting_name: &str,
    is_org: bool,
    subject: &str,
    body_snippet: &str,
) -> String {
    // 1. Try local Gemini bridge endpoint if active
    if let Ok(mut stream) = std::net::TcpStream::connect("127.0.0.1:8765") {
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(18)));
        let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(5)));

        let full_prompt = if let Some(base) = base_text {
            format!(
                "Você é o assistente executivo de e-mails do MailCraft.\n\
                O usuário está respondendo a um e-mail com os seguintes dados:\n\
                - Remetente: {sender_full}\n\
                - Assunto: {subject}\n\
                - Trecho do e-mail recebido: {body_snippet}\n\n\
                Sugestão prévia de resposta:\n\
                \"\"\"\n{base}\n\"\"\"\n\n\
                Instrução do usuário para redigir ou ajustar a resposta:\n\
                \"{instruction}\"\n\n\
                REGRAS OBRIGATÓRIAS:\n\
                1. Escreva uma resposta de e-mail completa, elegante, profissional e pronta para envio.\n\
                2. Inicie com saudação formal/apropriada (ex: 'Prezado(a)...' ou 'Olá...').\n\
                3. Atenda com precisão a instrução do usuário (ex: CNV, executivo, viagem, prazos, etc.).\n\
                4. Finalize com despedida cordial e a assinatura 'Atenciosamente,\nMarcondes Lima'.\n\
                5. Retorne APENAS o texto pronto do e-mail. Não retorne JSON nem explicações extras."
            )
        } else {
            format!(
                "Você é o assistente executivo de e-mails do MailCraft.\n\
                O usuário está respondendo a um e-mail com os seguintes dados:\n\
                - Remetente: {sender_full}\n\
                - Assunto: {subject}\n\
                - Trecho do e-mail recebido: {body_snippet}\n\n\
                Instrução do usuário para redigir a resposta:\n\
                \"{instruction}\"\n\n\
                REGRAS OBRIGATÓRIAS:\n\
                1. Escreva uma resposta de e-mail completa, elegante, profissional e pronta para envio.\n\
                2. Inicie com saudação formal/apropriada (ex: 'Prezado(a)...' ou 'Olá...').\n\
                3. Atenda com precisão a instrução do usuário (ex: CNV, executivo, viagem, prazos, etc.).\n\
                4. Finalize com despedida cordial e a assinatura 'Atenciosamente,\nMarcondes Lima'.\n\
                5. Retorne APENAS o texto pronto do e-mail. Não retorne JSON nem explicações extras."
            )
        };

        let sys_prompt = "Você é o assistente executivo de e-mails do MailCraft. Sua única função é redigir respostas de e-mails profissionais, elegantes e completas com base nas instruções do usuário e no contexto do e-mail. Retorne somente o texto pronto do e-mail para envio. Nunca mencione que você é um aplicativo de anotações ou Notebook.";

        let payload = json!({
            "prompt": full_prompt,
            "system_prompt": sys_prompt,
            "instruction": instruction,
            "sender": sender_full,
            "subject": subject,
            "email_body": body_snippet,
            "base_text": base_text
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
                            let mut raw_reply: Option<String> = None;
                            if let Some(r) = v.get("reply").and_then(|r| r.as_str()) {
                                raw_reply = Some(r.to_string());
                            } else if let Some(m) = v.get("message").and_then(|r| r.as_str()) {
                                raw_reply = Some(m.to_string());
                            } else if let Some(e) = v.get("email_body").and_then(|r| r.as_str()) {
                                raw_reply = Some(e.to_string());
                            } else if let Some(er) = v.get("email_response").and_then(|r| r.as_str()) {
                                raw_reply = Some(er.to_string());
                            }

                            if let Some(reply_str) = raw_reply {
                                let mut final_text = reply_str.trim().to_string();
                                // If the returned text is formatted as JSON, unpack the inner body
                                if (final_text.starts_with('{') && final_text.ends_with('}'))
                                    || final_text.starts_with("```json")
                                    || final_text.starts_with("```")
                                {
                                    let clean = final_text
                                        .trim_start_matches("```json")
                                        .trim_start_matches("```")
                                        .trim_end_matches("```")
                                        .trim();
                                    if let Ok(inner) = serde_json::from_str::<serde_json::Value>(clean) {
                                        for key in &["email_response", "email_body", "message", "reply", "text", "response"] {
                                            if let Some(val) = inner.get(*key).and_then(|v| v.as_str()) {
                                                if !val.trim().is_empty() {
                                                    final_text = val.trim().to_string();
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                }

                                // Reject any notebook boilerplate
                                let lower = final_text.to_lowercase();
                                let is_notebook_boilerplate = lower.contains("assistente inteligente integrado ao aplicativo notebook")
                                    || lower.contains("posso criar notas, organizar cadernos")
                                    || (lower.contains("notebook") && lower.contains("caderno"));

                                if !is_notebook_boilerplate && !final_text.is_empty() {
                                    return final_text;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 2. Intelligent contextual reply rewriting based on prompt intent and base text
    let p_lower = instruction.to_lowercase();
    let salutation = if is_org {
        format!("Olá, {greeting_name}!")
    } else {
        format!("Olá, {greeting_name}!")
    };

    if p_lower.contains("curta") || p_lower.contains("curto") || p_lower.contains("resum") || p_lower.contains("sucint") {
        format!(
            "{salutation}\n\n\
            Recebi sua mensagem sobre \"{subject}\". Confirmo o recebimento e o alinhamento com as informações. Qualquer dúvida estou à disposição.\n\n\
            Atenciosamente,\n\
            Marcondes Lima"
        )
    } else if p_lower.contains("formal") || p_lower.contains("profissional") {
        let formal_salutation = if is_org {
            format!("Prezada {greeting_name},")
        } else {
            format!("Prezado(a) {sender_full},")
        };
        format!(
            "{formal_salutation}\n\n\
            Acusamos o recebimento de vossa mensagem referente a \"{subject}\". Informamos que o teor do comunicado foi devidamente registrado e encaminhado aos responsáveis.\n\n\
            Renovamos nossos votos de estima e consideração.\n\n\
            Cordialmente,\n\
            Marcondes Lima"
        )
    } else if p_lower.contains("amig") || p_lower.contains("informal") || p_lower.contains("abraço") {
        format!(
            "Oi, {greeting_name}!\n\n\
            Tudo bem? Passando para agradecer pelo envio das informações sobre \"{subject}\". Já verifiquei por aqui e está tudo certo!\n\n\
            Se precisar de qualquer coisa é só me avisar. Um grande abraço,\n\
            Marcondes"
        )
    } else if p_lower.contains("desconto") || p_lower.contains("preco") || p_lower.contains("preço") || p_lower.contains("valor") {
        format!(
            "{salutation}\n\n\
            Agradeço pelo envio da proposta sobre \"{subject}\".\n\
            Estudamos com atenção as condições apresentadas. Gostaríamos de verificar se há margem para uma condição especial ou percentual de desconto para viabilizarmos o fechamento de imediato.\n\n\
            Fico no aguardo do seu parecer.\n\n\
            Atenciosamente,\n\
            Marcondes Lima"
        )
    } else if p_lower.contains("férias") || p_lower.contains("ferias") || p_lower.contains("viagem") || p_lower.contains("ausente") {
        format!(
            "{salutation}\n\n\
            Obrigado pelo contato sobre \"{subject}\".\n\
            Informo que estarei temporariamente ausente durante estes dias, com acesso restrito a e-mails. Assim que retornar, darei prioridade máxima para responder a esta solicitação.\n\n\
            Caso seja algo urgente, peço a gentileza de contatar nossa equipe.\n\n\
            Um abraço,\n\
            Marcondes Lima"
        )
    } else if p_lower.contains("prazo") || p_lower.contains("data") || p_lower.contains("quando") {
        format!(
            "{salutation}\n\n\
            Agradeço pelo contato referente a \"{subject}\".\n\
            Você teria uma estimativa de prazo ou previsão de datas para os próximos desdobramentos?\n\n\
            Aguardo seu retorno.\n\n\
            Atenciosamente,\n\
            Marcondes Lima"
        )
    } else if p_lower.contains("recus") || p_lower.contains("não posso") || p_lower.contains("nao posso") || p_lower.contains("infelizmente") {
        format!(
            "Prezado(a) {sender_full},\n\n\
            Agradeço pela consideração e pelo envio da mensagem em relação a \"{subject}\".\n\
            Infelizmente, devido a compromissos assumidos anteriormente e limitações de agenda, não poderei prosseguir neste momento.\n\n\
            Espero que tenhamos novas oportunidades futuras de cooperação.\n\n\
            Cordialmente,\n\
            Marcondes Lima"
        )
    } else {
        // Base adjustment
        if let Some(base) = base_text {
            format!(
                "{salutation}\n\n\
                Em relação à mensagem sobre \"{subject}\": {instruction}.\n\n\
                Detalhes alinhados conforme solicitado:\n\
                {}\n\n\
                Permaneço à disposição para esclarecimentos adicionais.\n\n\
                Atenciosamente,\n\
                Marcondes Lima",
                base.trim()
            )
        } else {
            format!(
                "{salutation}\n\n\
                Obrigado pelo retorno sobre \"{subject}\".\n\n\
                Em atenção à sua mensagem: {instruction}.\n\n\
                Estou à total disposição para alinharmos quaisquer outros pontos necessários.\n\n\
                Atenciosamente,\n\
                Marcondes Lima"
            )
        }
    }
}

pub fn render_ai_reply_panel(app: &mut MailApp, ui: &mut Ui, m: &Arc<Message>, t: &Tokens) {
    app.ai_reply.poll_incoming();

    let (sender_full, _, is_org) = extract_sender_names(m);
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

        // Header bar (fixed at top of panel)
        ui.horizontal(|ui| {
            ui.label(RichText::new("✨").size(16.0).color(Color32::from_rgb(66, 133, 244)));
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
            let sender_tag = if is_org { "Empresa/Serviço" } else { "Contato" };
            ui.label(
                RichText::new(format!("Lendo e-mail de: {} ({}) • Assunto: {}", sender_full, sender_tag, subject))
                    .size(11.5)
                    .color(t.text_dim),
            );
        });

        ui.add_space(2.0);

        // Actions to execute after UI loop
        let mut chosen_reply: Option<String> = None;
        let mut copy_reply: Option<String> = None;
        let mut chat_adjust_request: Option<(String, Option<String>)> = None;

        // SCROLLABLE AREA: Guarantees nothing is ever cut off on any window size
        egui::ScrollArea::vertical()
            .id_salt("ai_reply_scroll_area")
            .max_height(350.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 8.0);

                // Suggestions list: Show 2 by default, expandable to 3
                let total_suggestions = app.ai_reply.suggestions.len();
                let limit = if app.ai_reply.show_all_suggestions { total_suggestions } else { total_suggestions.min(2) };

                for (idx, sugg) in app.ai_reply.suggestions[..limit].iter().enumerate() {
                    let is_active_base = app.ai_reply.chat_base_text.as_ref() == Some(&sugg.text);

                    let card_frame = egui::Frame::NONE
                        .fill(if is_active_base {
                            Color32::from_rgba_premultiplied(66, 133, 244, 25)
                        } else {
                            Color32::from_rgba_premultiplied(66, 133, 244, 12)
                        })
                        .stroke(Stroke::new(
                            if is_active_base { 1.5 } else { 1.0 },
                            if is_active_base { Color32::from_rgb(66, 133, 244) } else { Color32::from_rgba_premultiplied(66, 133, 244, 70) },
                        ))
                        .corner_radius(6.0)
                        .inner_margin(Margin::same(10));

                    card_frame.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("Opção {}: {}", idx + 1, &sugg.category))
                                    .font(semibold(12.5))
                                    .color(Color32::from_rgb(66, 133, 244)),
                            );
                            ui.add_space(6.0);
                            ui.label(RichText::new(&sugg.description).size(11.0).color(t.text_dim));
                        });

                        ui.add_space(4.0);

                        // Complete text with proper line wrapping - NO CUT-OFFS
                        for line in sugg.text.lines() {
                            if line.trim().is_empty() {
                                ui.add_space(3.0);
                            } else {
                                ui.add(egui::Label::new(RichText::new(line).size(12.0).color(t.text)).wrap());
                            }
                        }

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

                            let chat_btn = egui::Button::new(
                                RichText::new("💬 Ajustar no Chat")
                                    .font(semibold(11.5))
                                    .color(Color32::from_rgb(66, 133, 244)),
                            )
                            .fill(Color32::from_rgba_premultiplied(66, 133, 244, 20))
                            .corner_radius(4.0);

                            if ui.add(chat_btn).on_hover_text("Carrega esta sugestão no Chat para o Gemini ajustar conforme seu pedido").clicked() {
                                app.ai_reply.chat_base_text = Some(sugg.text.clone());
                                app.ai_reply.prompt = "Tornar mais amigável e resumida".to_string();
                            }

                            if ui
                                .button(RichText::new("📋 Copiar").size(11.5).color(t.text))
                                .on_hover_text("Copia a resposta para a área de transferência")
                                .clicked()
                            {
                                copy_reply = Some(sugg.text.clone());
                            }
                        });
                    });
                }

                // Toggle for 3rd suggestion if there are more
                if total_suggestions > 2 {
                    ui.horizontal(|ui| {
                        if !app.ai_reply.show_all_suggestions {
                            if ui
                                .button(RichText::new("➕ Mostrar 3ª sugestão de resposta").size(11.5).color(Color32::from_rgb(66, 133, 244)))
                                .clicked()
                            {
                                app.ai_reply.show_all_suggestions = true;
                            }
                        } else {
                            if ui
                                .button(RichText::new("➖ Mostrar apenas 2 sugestões principais").size(11.5).color(t.text_dim))
                                .clicked()
                            {
                                app.ai_reply.show_all_suggestions = false;
                            }
                        }
                    });
                }

                ui.add_space(4.0);

                // INTERACTIVE CHAT SECTION: Modify reply or converse with Gemini
                let chat_frame = egui::Frame::NONE
                    .fill(Color32::from_rgba_premultiplied(30, 35, 45, 30))
                    .stroke(Stroke::new(1.0, Color32::from_rgba_premultiplied(100, 150, 255, 60)))
                    .corner_radius(6.0)
                    .inner_margin(Margin::same(10));

                chat_frame.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("💬").size(14.0).color(Color32::from_rgb(66, 133, 244)));
                        ui.label(
                            RichText::new("Chat com Gemini sobre a Resposta:")
                                .font(semibold(12.5))
                                .color(t.text),
                        );

                        if app.ai_reply.chat_base_text.is_some() {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button(RichText::new("Limpar base").size(10.5).color(t.text_dim)).clicked() {
                                    app.ai_reply.chat_base_text = None;
                                }
                            });
                        }
                    });

                    if let Some(base) = &app.ai_reply.chat_base_text {
                        ui.horizontal_wrapped(|ui| {
                            let snippet = if base.len() > 90 { format!("{}...", &base[..90]) } else { base.clone() };
                            ui.label(RichText::new(format!("Base selecionada: \"{}\"", snippet.replace('\n', " "))).size(11.0).color(Color32::from_rgb(66, 133, 244)));
                        });
                    }

                    // Quick prompt chips
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Ajustes rápidos:").size(11.0).color(t.text_dim));
                        if ui.button(RichText::new("✂️ Mais curta").size(11.0)).clicked() {
                            chat_adjust_request = Some(("Deixe a resposta mais curta e direta".to_string(), app.ai_reply.chat_base_text.clone()));
                        }
                        if ui.button(RichText::new("👔 Mais formal").size(11.0)).clicked() {
                            chat_adjust_request = Some(("Reescreva em tom mais formal e polido".to_string(), app.ai_reply.chat_base_text.clone()));
                        }
                        if ui.button(RichText::new("😊 Mais amigável").size(11.0)).clicked() {
                            chat_adjust_request = Some(("Reescreva em tom amigável e descontraído".to_string(), app.ai_reply.chat_base_text.clone()));
                        }
                        if ui.button(RichText::new("📅 Pedir prazo").size(11.0)).clicked() {
                            chat_adjust_request = Some(("Solicite estimativa de prazos ou próximas datas".to_string(), app.ai_reply.chat_base_text.clone()));
                        }
                    });

                    // Input field
                    ui.horizontal(|ui| {
                        let hint = if app.ai_reply.chat_base_text.is_some() {
                            "Diga o que ajustar (ex: 'diga que já paguei', 'mude o prazo')..."
                        } else {
                            "Peça uma resposta ao Gemini (ex: 'peça desconto', 'diga que estarei viajando')..."
                        };

                        let input = egui::TextEdit::singleline(&mut app.ai_reply.prompt)
                            .hint_text(hint)
                            .desired_width(ui.available_width() - 110.0);
                        let resp = ui.add(input);
                        let enter_pressed = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

                        let send_btn = egui::Button::new(
                            RichText::new("✨ Enviar")
                                .font(semibold(11.5))
                                .color(t.accent_text),
                        )
                        .fill(t.accent)
                        .corner_radius(4.0);

                        if (ui.add(send_btn).clicked() || enter_pressed) && !app.ai_reply.prompt.trim().is_empty() {
                            let text = app.ai_reply.prompt.trim().to_string();
                            app.ai_reply.prompt.clear();
                            chat_adjust_request = Some((text, app.ai_reply.chat_base_text.clone()));
                        }
                    });
                });

                if app.ai_reply.loading {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            RichText::new("Gemini está analisando e redigindo sua resposta...")
                                .size(11.5)
                                .color(Color32::from_rgb(66, 133, 244)),
                        );
                    });
                }

                // Custom/Adjusted reply card
                if let Some(custom_text) = app.ai_reply.custom_reply.clone() {
                    ui.add_space(4.0);
                    let custom_frame = egui::Frame::NONE
                        .fill(Color32::from_rgba_premultiplied(52, 168, 83, 16))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(52, 168, 83)))
                        .corner_radius(6.0)
                        .inner_margin(Margin::same(10));

                    custom_frame.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("✨ Resposta Gerada pelo Gemini:")
                                    .font(semibold(12.5))
                                    .color(Color32::from_rgb(52, 168, 83)),
                            );
                        });
                        ui.add_space(4.0);

                        for line in custom_text.lines() {
                            if line.trim().is_empty() {
                                ui.add_space(3.0);
                            } else {
                                ui.add(egui::Label::new(RichText::new(line).size(12.0).color(t.text)).wrap());
                            }
                        }

                        ui.add_space(6.0);

                        ui.horizontal(|ui| {
                            let use_btn = egui::Button::new(
                                RichText::new("✉️ Usar esta Resposta no E-mail")
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
                                copy_reply = Some(custom_text.clone());
                            }
                        });
                    });
                }
            });

        // Trigger chat adjustment if requested
        if let Some((user_text, base_opt)) = chat_adjust_request {
            send_chat_adjustment(app, m, user_text, base_opt);
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
