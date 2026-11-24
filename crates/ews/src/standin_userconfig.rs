// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The stand-in's `GetUserConfiguration` (#484): the one configuration the client asks for,
//! `OWA.UserOptions` in the root folder, with the dictionary a test gave it. Held to the schema:
//! the name and the folder it is in, and the properties asked for.

use crate::soap::{MESSAGES, TYPES};
use crate::standin::{Check, Refusal, State, error_message, invalid, response};
use crate::standin_mail::ok_message;
use crate::xml::{Node, escape, escape_text};

impl State {
    pub(crate) fn get_user_configuration(&self, operation: &Node) -> Result<(u16, String, String), Refusal> {
        Check::is(operation, MESSAGES, "GetUserConfiguration", &[])?;
        let parts = Check::children(operation, MESSAGES, &["UserConfigurationName", "UserConfigurationProperties"])?;
        let name = Check::required(operation, parts.first().copied().flatten(), "UserConfigurationName")?;
        Check::is(name, MESSAGES, "UserConfigurationName", &["Name"])?;
        let [folder] = name.children.as_slice() else { return Err(invalid("UserConfigurationName names one folder")) };
        Check::is(folder, TYPES, "DistinguishedFolderId", &["Id", "ChangeKey"])?;
        Check::empty(folder)?;
        let properties = Check::required(operation, parts.get(1).copied().flatten(), "UserConfigurationProperties")?;
        Check::is(properties, MESSAGES, "UserConfigurationProperties", &[])?;
        if !["Id", "Dictionary", "XmlData", "BinaryData", "All"].contains(&properties.text.trim()) {
            return Err(invalid("UserConfigurationProperties is Id, Dictionary, XmlData, BinaryData or All"));
        }
        let (asked, at) = (Check::attribute(name, "Name")?, Check::attribute(folder, "Id")?);
        let Some(entries) = self.user_options.as_ref().filter(|_| asked == "OWA.UserOptions" && at == "root") else {
            let message = error_message("GetUserConfiguration", "ErrorItemNotFound");
            return Ok((200, "ErrorItemNotFound".into(), response("GetUserConfiguration", &message)));
        };
        let mut dictionary = String::new();
        for (key, kind, values) in entries {
            let values: String = values.iter().map(|v| format!("<t:Value>{}</t:Value>", escape_text(v))).collect();
            dictionary.push_str(&format!(
                "<t:DictionaryEntry><t:DictionaryKey><t:Type>String</t:Type><t:Value>{}</t:Value></t:DictionaryKey>\
                 <t:DictionaryValue><t:Type>{}</t:Type>{values}</t:DictionaryValue></t:DictionaryEntry>",
                escape(key),
                escape(kind)
            ));
        }
        let inner = format!(
            "<m:UserConfiguration><t:UserConfigurationName Name=\"OWA.UserOptions\"><t:DistinguishedFolderId Id=\"root\"/></t:UserConfigurationName>\
             <t:Dictionary>{dictionary}</t:Dictionary></m:UserConfiguration>"
        );
        Ok((200, "NoError".into(), response("GetUserConfiguration", &ok_message("GetUserConfiguration", &inner))))
    }
}
