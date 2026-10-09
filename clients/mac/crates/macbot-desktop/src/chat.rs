use super::*;
use gpui_kit::prelude::FluentBuilder;

impl MacBot {
    pub(super) fn send_message(&mut self,window:&mut Window,cx:&mut Context<Self>) {
        if !self.connected {self.notice=tr("error.offline").to_string();cx.notify();return;}
        let text=self.composer.read(cx).value().to_string();
        if text.trim().is_empty()&&self.attachments.is_empty(){return;}
        if let Some(project)=self.changes_project.take(){self.rpc("project.request_changes",json!({"project_id":project,"text":text}),cx);}else{
            let mut mentions=vec![];
            for bot in self.state.bots.values(){if text.contains(&format!("@{}",s(bot,"name"))){mentions.push(json!({"kind":"bot","bot_id":bot["id"],"instruction":null}));}}
            if text.contains("@everyone"){mentions=vec![json!({"kind":"everyone"})];}
            self.rpc("chat.send",json!({"chat_id":self.selected_chat,"text":text,"mentions":mentions,"reply_to":self.reply_to,"attachments":self.attachments}),cx);
        }
        self.composer.update(cx,|input,cx|input.set_value("",window,cx));self.drafts.remove(&self.selected_chat);self.attachments.clear();self.reply_to=None;cx.notify();
    }
    pub(super) fn chat_page(&self,_window:&mut Window,cx:&mut Context<Self>)->AnyElement {
        let t=Tokens::get(cx);let chat=self.state.chats.get(&self.selected_chat).cloned().unwrap_or(Value::Null);
        let bot=self.state.bots.get(s(&chat,"bot_id")).cloned().unwrap_or(Value::Null);
        let mut header=div().h(px(64.)).flex_none().px_6().border_b_1().border_color(t.border).flex().items_center().justify_between()
            .child(div().flex().gap_3().items_center().child(bean(&bot,36.,cx)).child(div().flex().flex_col().gap_1().child(div().font_weight(FontWeight::SEMIBOLD).child(s(&chat,"title").to_string())).when(s(&chat,"kind")=="main",|el|el.child(div().text_xs().text_color(t.secondary).child(tr("chat.main"))))));
        header=header.child(div().flex().gap_1().child(Button::new("chat-trace").ghost().icon(IconName::FileText).tooltip(tr("action.full_trace")).on_click(cx.listener(|this,_,_,cx|this.open_trace(None,cx)))).child(Button::new("chat-context").ghost().icon(IconName::PanelRight).tooltip(tr("context.title")).on_click(cx.listener(|this,_,_,cx|{this.context_visible=!this.context_visible;cx.notify();}))));
        let mut contents=div().id("messages").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.message_scroll).px_6().py_4().flex().flex_col().gap_4();
        let mut messages:Vec<_>=self.state.messages.values().filter(|v|s(v,"chat_id")==self.selected_chat&&v["deleted"]!=true&&v["reply_to"].is_null()).collect();messages.sort_by_key(|v|v["seq"].as_u64().unwrap_or(0));
        if let Some(first)=messages.first(){let seq=first["seq"].as_u64().unwrap_or(0);let id=self.selected_chat.clone();contents=contents.child(Button::new("history-more").ghost().small().label(tr("chat.more")).on_click(cx.listener(move|this,_,_,cx|this.rpc("chat.history",json!({"chat_id":id,"before_seq":seq}),cx))));}
        if messages.is_empty(){contents=contents.child(div().flex_1().flex().flex_col().gap_3().items_center().justify_center().text_color(t.secondary).child(tr("chat.empty")).child(div().text_xs().child(tr("chat.empty_hint"))));}
        for message in messages{contents=contents.child(self.message_row(message,cx));}
        let readonly=s(&chat,"kind")=="bot_dm";
        let mut composer=div().flex_none().border_t_1().border_color(t.border).p_4().flex().flex_col().gap_2();
        if let Some(root)=&self.reply_to{composer=composer.child(div().text_xs().text_color(t.accent).child(format!("{} · {}",tr("chat.reply"),root)));}
        let text=self.composer.read(cx).value().to_string();
        if let Some(last)=text.split_whitespace().last(){
            if last.starts_with('@'){
                let mut choices=div().flex().flex_wrap().gap_1();
                for bot in self.state.bots.values().filter(|b|s(&chat,"kind")!="project"||arr(&chat,"member_bot_ids").iter().any(|id|id==&b["id"])) {let name=s(bot,"name").to_string();let prefix=last.to_string();choices=choices.child(Button::new(SharedString::from(format!("mention-{}",s(bot,"id")))).ghost().small().label(format!("@{name}")).on_click(cx.listener(move|this,_,window,cx|{let text=this.composer.read(cx).value().to_string();let keep=text.len().saturating_sub(prefix.len());let result=format!("{}@{} ",&text[..keep],name);this.composer.update(cx,|input,cx|input.set_value(result,window,cx));})));}
                composer=composer.child(choices);
            }
        }
        composer=composer.child(div().rounded(px(22.)).bg(t.sidebar).flex().items_end().gap_2().p_2().child(Button::new("attachment").ghost().icon(IconName::Plus).tooltip(tr("chat.attach")).disabled(!self.connected||readonly).on_click(cx.listener(|this,_,window,cx|this.attach(window,cx))))
            .child(Textarea::new(&self.composer).disabled(!self.connected||readonly).flex_1())
            .child(Button::new("send").primary().icon(IconName::ArrowUp).tooltip(tr("chat.send")).disabled(!self.connected||readonly).on_click(cx.listener(|this,_,window,cx|this.send_message(window,cx)))));
        if !self.attachments.is_empty(){composer=composer.child(div().text_xs().text_color(t.accent).child(format!("{} · {}",tr("notice.attachment"),self.attachments.len())));}
        div().size_full().flex().flex_col().child(header)
            .when(s(&chat,"kind")=="project",|el|el.child(self.project_status(&chat,cx)))
            .when(!self.connected&&!self.fixture,|el|el.child(div().px_4().py_2().text_xs().text_color(t.attention).child(tr("status.disconnected"))))
            .child(contents).child(composer).into_any_element()
    }
    fn project_status(&self,chat:&Value,cx:&mut Context<Self>)->AnyElement {
        let t=Tokens::get(cx);let project=self.state.projects.get(s(chat,"project_id")).cloned().unwrap_or(Value::Null);let ann=self.state.announcements.get(s(chat,"project_id")).cloned().unwrap_or(Value::Null);
        let mut row=div().flex().items_center().gap_2().px_4().py_2().border_b_1().border_color(t.border);
        for member in arr(&ann,"members"){let bot=self.state.bots.get(s(member,"bot_id")).cloned().unwrap_or(Value::Null);let assignment=member["current_assignment_id"].as_str().map(str::to_string);row=row.child(Button::new(SharedString::from(format!("status-{}",s(member,"bot_id")))).ghost().small().label(format!("{} · {}",s(&bot,"name"),state(s(member,"state")))).on_click(cx.listener(move|this,_,_,cx|this.open_trace(assignment.clone(),cx))));}
        let project_id=s(&project,"id").to_string();
        row=row.child(Button::new("announcement").ghost().small().label(tr("context.announcement")).on_click(cx.listener(|this,_,_,cx|{this.context.clear();this.context_visible=true;cx.notify();}))).child(div().flex_1());
        if s(&project,"status")=="review"{row=row.child(self.rpc_button("confirm-project","action.confirm","project.confirm_done",json!({"project_id":project_id}),cx));}
        div().flex().flex_col().child(row).child(div().px_4().py_1().text_xs().text_color(t.secondary).child(format!("{} · {}",s(&project,"goal"),s(&project,"home_path")))).into_any_element()
    }
    fn message_row(&self,message:&Value,cx:&mut Context<Self>)->AnyElement {
        let t=Tokens::get(cx);let user=s(&message["sender"],"kind")=="user";let system=s(&message["sender"],"kind")=="system";
        let bot=self.state.bots.get(s(&message["sender"],"bot_id")).cloned().unwrap_or(Value::Null);let id=s(message,"id").to_string();
        let mut bubble=div().flex().flex_col().gap_3().px_4().py_3().rounded(px(18.)).max_w(px(620.)).bg(if user{t.user}else{t.bot}).text_color(if user{t.user_text}else{t.primary});
        if message["streaming"]==true{bubble=bubble.child(TextView::markdown(SharedString::from(format!("stream-{id}")),s(message,"fallback_text").to_string()));}else{
            let blocks=arr(message,"blocks");if blocks.is_empty(){bubble=bubble.child(s(message,"fallback_text").to_string());}
            for (index,block) in blocks.iter().enumerate(){bubble=bubble.child(self.block(message,block,&format!("{id}-{index}"),cx));}
        }
        let mut message_content=div().flex().flex_col().gap_1().max_w(px(660.)).when(!user&&!system,|el|el.child(div().text_xs().text_color(t.secondary).child(s(&bot,"name").to_string()))).child(bubble);
        for delivery in arr(message,"delivery"){let name=self.state.bots.get(s(delivery,"bot_id")).map(|b|s(b,"name")).unwrap_or("");message_content=message_content.child(div().text_xs().text_color(t.secondary).child(format!("{name} {}",tr(match s(delivery,"state"){"read"=>"chat.read","delivered"=>"chat.delivered",_=>"chat.queued"}))));}
        let root=id.clone();let chat=self.selected_chat.clone();let copy=s(message,"fallback_text").to_string();let react=id.clone();
        message_content=message_content.child(div().flex().gap_1()
            .child(Button::new(SharedString::from(format!("reply-{id}"))).ghost().xsmall().label(tr("chat.reply")).on_click(cx.listener(move|this,_,_,cx|{this.reply_to=Some(root.clone());this.rpc("chat.thread",json!({"chat_id":chat,"root_message_id":root}),cx);})))
            .child(Button::new(SharedString::from(format!("copy-{id}"))).ghost().xsmall().label(tr("chat.copy")).on_click(move|_,_,cx|cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))))
            .child(Button::new(SharedString::from(format!("react-{id}"))).ghost().xsmall().label("☺").on_click(cx.listener(move|this,_,_,cx|this.rpc("chat.react",json!({"message_id":react,"emoji":"👍","on":true}),cx)))));
        div().id(SharedString::from(id)).flex().gap_3().when(user,|el|el.justify_end()).when(system,|el|el.justify_center()).when(!user&&!system,|el|el.child(bean(&bot,28.,cx))).child(message_content).into_any_element()
    }
    fn block(&self,message:&Value,block:&Value,id:&str,cx:&mut Context<Self>)->AnyElement {
        let t=Tokens::get(cx);let mut body=div().flex().flex_col().gap_2();let kind=s(block,"type");
        match kind {
            "text"=>return TextView::markdown(SharedString::from(format!("md-{id}")),s(block,"markdown").to_string()).into_any_element(),
            "progress"=>body=body.child(div().text_xs().text_color(t.secondary).child(s(block,"text").to_string())),
            "system"=>body=body.child(div().text_xs().text_color(t.secondary).child(s(block,"text").to_string())),
            "blocked"=>body=body.child(div().text_color(t.danger).child(format!("{} · {}",tr("block.blocked"),s(block,"reason")))),
            "project_card"|"review_card"=>{
                let project=self.state.projects.get(s(block,"project_id")).cloned().unwrap_or(Value::Null);let project_id=s(block,"project_id").to_string();let chat=s(&project,"chat_id").to_string();
                body=body.child(div().font_weight(FontWeight::SEMIBOLD).child(format!("{} · {}",tr(if kind=="review_card"{"block.review"}else{"block.project"}),s(&project,"name")))).child(s(&project,"goal").to_string());
                for artifact in arr(block,"artifacts"){body=body.child(format!("▤ {} · {}",s(artifact,"title"),s(artifact,"path_or_url")));}
                if kind=="review_card"&&s(block,"state")=="pending"{let project_copy=project_id.clone();body=body.child(self.rpc_button(&format!("confirm-{id}"),"action.confirm","project.confirm_done",json!({"project_id":project_id}),cx)).child(Button::new(SharedString::from(format!("changes-{id}"))).outline().small().label(tr("action.changes")).on_click(cx.listener(move|this,_,window,cx|{this.changes_project=Some(project_copy.clone());this.notice=tr("notice.changes").to_string();this.composer.update(cx,|input,cx|input.focus(window,cx));cx.notify();})));}
                body=body.child(Button::new(SharedString::from(format!("enter-{id}"))).ghost().small().label(tr("action.enter")).on_click(cx.listener(move|this,_,window,cx|this.select_chat(chat.clone(),window,cx))));
            },
            "task_card"=>{let aid=s(block,"assignment_id").to_string();let assignment=self.state.assignments.get(&aid).cloned().unwrap_or(Value::Null);let status=s(&assignment,"status");body=body.child(format!("{} · {}",state(status),s(&assignment,"title"))).child(Button::new(SharedString::from(format!("details-{id}"))).ghost().small().label(tr("action.details")).on_click(cx.listener(move|this,_,_,cx|this.open_trace(Some(aid.clone()),cx))));},
            "completion"=>{body=body.child(div().font_weight(FontWeight::SEMIBOLD).child(s(block,"summary").to_string()));for artifact in arr(block,"artifacts"){body=body.child(self.artifact_row(artifact,&format!("{id}-{}",s(artifact,"artifact_id")),cx));}for next in arr(block,"next"){body=body.child(format!("@{} {}",self.state.bots.get(s(next,"bot_id")).map(|b|s(b,"name")).unwrap_or("Bot"),s(next,"instruction")));}},
            "approval"=>{let approval=self.state.approvals.get(s(block,"approval_id")).cloned().unwrap_or(Value::Null);body=body.child(format!("{} · {}",tr("block.approval"),s(&approval,"tool"))).child(s(&approval,"summary").to_string()).child(TextView::markdown(SharedString::from(format!("approval-md-{id}")),s(&approval,"detail").to_string()));if s(&approval,"state")=="pending"{for decision in ["allow_once","always_allow","deny"]{body=body.child(self.rpc_button(&format!("{decision}-{id}"),&format!("action.{decision}"),"approval.decide",json!({"approval_id":approval["id"],"decision":decision}),cx));}}else{body=body.child(s(&approval,"state").to_string());}},
            "approval_ref"|"bot_dm_ref"=>{let target=s(block,"chat_id").to_string();body=body.child(tr(if kind=="approval_ref"{"block.approval_ref"}else{"block.dm"})).child(Button::new(SharedString::from(format!("ref-{id}"))).ghost().small().label(tr("action.open")).on_click(cx.listener(move|this,_,window,cx|this.select_chat(target.clone(),window,cx))));},
            "question"=>{let q=self.state.questions.get(s(block,"question_id")).cloned().unwrap_or(Value::Null);body=body.child(s(&q,"text").to_string());for (index,option) in arr(&q,"options").iter().enumerate(){body=body.child(self.rpc_button(&format!("option-{id}-{index}"),option.as_str().unwrap_or(""),"question.answer",json!({"question_id":q["id"],"option_index":index}),cx));}},
            "delegation"=>{let aid=s(block,"assignment_id").to_string();body=body.child(format!("↪ {} {}",tr("block.delegation"),self.state.bots.get(s(block,"bot_id")).map(|b|s(b,"name")).unwrap_or("Bot"))).child(Button::new(SharedString::from(format!("delegation-{id}"))).ghost().small().label(tr("action.details")).on_click(cx.listener(move|this,_,_,cx|this.open_trace(Some(aid.clone()),cx))));},
            "loop_paused"=>{body=body.child(tr("block.loop"));for action in ["continue","end"]{body=body.child(self.rpc_button(&format!("loop-{id}-{action}"),&format!("action.{action}"),"loop.resolve",json!({"root_message_id":block["root_message_id"],"action":action}),cx));}},
            "takeover_request"=>{let bot=s(block,"bot_id").to_string();body=body.child(s(block,"reason").to_string()).child(Button::new(SharedString::from(format!("takeover-{id}"))).outline().small().label(tr("action.takeover")).on_click(cx.listener(move|this,_,_,cx|this.open_computer(bot.clone(),cx))));},
            "file"|"image"=>{let file=block["file"].clone();body=body.child(format!("▤ {}",s(&file,"name"))).child(Button::new(SharedString::from(format!("file-{id}"))).ghost().small().label(tr("action.open")).on_click(cx.listener(move|this,_,_,cx|this.download_file(file.clone(),cx))));},
            _=>body=body.child(s(message,"fallback_text").to_string())
        }
        body.into_any_element()
    }
    fn rpc_button(&self,id:&str,label:&str,method:&str,params:Value,cx:&mut Context<Self>)->Button {
        let method=method.to_owned();Button::new(SharedString::from(id.to_owned())).outline().small().label(tr(label)).disabled(!self.connected).on_click(cx.listener(move|this,_,_,cx|this.rpc(&method,params.clone(),cx)))
    }
    fn artifact_row(&self,artifact:&Value,id:&str,cx:&mut Context<Self>)->AnyElement {
        let path=s(artifact,"path_or_url").to_string();let project=self.state.projects.get(s(artifact,"project_id")).cloned().unwrap_or(Value::Null);let file=json!({"root":"project","root_id":project["id"],"path":path,"name":s(artifact,"title")});
        div().flex().flex_col().gap_1().child(format!("▤ {}",s(artifact,"title"))).child(div().text_xs().text_color(Tokens::get(cx).code).child(path.clone())).child(Button::new(SharedString::from(format!("artifact-{id}"))).ghost().small().label(tr("action.open")).on_click(cx.listener(move|this,_,_,cx|{if path.starts_with("http://")||path.starts_with("https://"){cx.open_url(&path);}else{this.download_file(file.clone(),cx);}}))).into_any_element()
    }
    pub(super) fn context_panel(&self,_window:&mut Window,cx:&mut Context<Self>)->AnyElement {
        let t=Tokens::get(cx);let title=match self.context.last().map(String::as_str){Some("trace")=>if self.timeline.live{"trace.live"}else{"context.replay"},Some("thread")=>"chat.thread",_=>"context.title"};
        let header=div().flex().items_center().justify_between().p_3().border_b_1().border_color(t.border).child(Button::new("context-back").ghost().icon(IconName::ChevronLeft).on_click(cx.listener(|this,_,window,cx|this.back(window,cx)))).child(tr(title)).child(Button::new("context-close").ghost().icon(IconName::ChevronRight).on_click(cx.listener(|this,_,_,cx|{this.context_visible=false;this.close_trace(cx);cx.notify();})));
        let mut body=div().id("context-content").flex_1().min_h_0().overflow_y_scroll().p_4().flex().flex_col().gap_4();
        if self.context.last().is_some_and(|p|p=="trace"){
            if self.timeline.has_more_before{let mut params=self.trace_target.clone();params["before_aseq"]=json!(self.timeline.first_aseq);body=body.child(self.rpc_button("trace-more","trace.load","trace.history",params,cx));}
            if let Some(assignment)=self.state.assignments.get(s(&self.trace_target,"assignment_id")){body=body.child(div().font_weight(FontWeight::SEMIBOLD).child(s(assignment,"title").to_string())).child(format!("{} · {} tok",state(s(assignment,"status")),assignment["usage"]["input_tokens"].as_u64().unwrap_or(0)+assignment["usage"]["output_tokens"].as_u64().unwrap_or(0))).child(self.rpc_button("stop-task","action.stop","assignment.stop",json!({"assignment_id":assignment["id"]}),cx)).child(tr("context.instruction")).child(s(assignment,"instruction").to_string());for steer in arr(assignment,"steers"){body=body.child(format!("✎ {}",s(steer,"text")));}}
            for (aseq,item) in &self.timeline.items {let data=&item["data"];let kind=s(item,"type");let text=match kind{"llm.response"=>s(data,"text"),"tool.end"=>s(data,"preview"),"steer"=>s(data,"text"),"tool.start"=>s(data,"name"),_=>s(data,"model")};body=body.child(div().flex().flex_col().gap_1().py_2().border_b_1().border_color(t.border).child(format!("{aseq} · {kind}")).child(div().text_xs().text_color(t.secondary).child(text.to_string())));}
            if self.timeline.items.is_empty(){body=body.child(tr("trace.empty"));}
        }else if self.context.last().is_some_and(|p|p=="thread"){
            if let Some(thread)=&self.thread{body=body.child(self.message_row(&thread["root"],cx));for message in arr(thread,"replies"){body=body.child(self.message_row(message,cx));}}
        }else if let Some(chat)=self.state.chats.get(&self.selected_chat){
            if s(chat,"kind")=="project"{
                let project=self.state.projects.get(s(chat,"project_id")).cloned().unwrap_or(Value::Null);let ann=self.state.announcements.get(s(chat,"project_id")).cloned().unwrap_or(Value::Null);
                body=body.child(div().font_weight(FontWeight::SEMIBOLD).child(tr("context.announcement"))).child(tr("context.home")).child(div().font_family("SF Mono").text_xs().text_color(t.code).child(s(&project,"home_path").to_string())).child(tr("context.goal")).child(s(&project,"goal").to_string()).child(tr("context.flow")).child(arr(&project,"flow").iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" → ")).child(tr("context.members"));
                for member in arr(&ann,"members"){let bot=self.state.bots.get(s(member,"bot_id")).cloned().unwrap_or(Value::Null);body=body.child(format!("{} · {}",s(&bot,"name"),state(s(member,"state"))));}
                body=body.child(tr("context.artifacts"));for artifact in arr(&ann,"artifacts"){body=body.child(self.artifact_row(artifact,s(artifact,"id"),cx));}
                body=body.child(tr("context.highlights"));for highlight in arr(&ann,"highlights"){body=body.child(div().text_sm().child(s(highlight,"text").to_string()));}
                body=body.child(self.rpc_button("archive-project","action.archive","project.archive",json!({"project_id":project["id"]}),cx));
            }else{
                let bot_id=s(chat,"bot_id");let bot=self.state.bots.get(bot_id).cloned().unwrap_or(Value::Null);body=body.child(bean(&bot,48.,cx)).child(div().font_weight(FontWeight::SEMIBOLD).child(s(&bot,"name").to_string())).child(tr("context.running"));
                for assignment in self.state.assignments.values().filter(|a|s(a,"bot_id")==bot_id){let id=s(assignment,"id").to_string();body=body.child(Button::new(SharedString::from(format!("task-{id}"))).ghost().label(format!("{} · {}",state(s(assignment,"status")),s(assignment,"title"))).on_click(cx.listener(move|this,_,_,cx|this.open_trace(Some(id.clone()),cx))));}
                body=body.child(tr("context.groups"));for project in self.state.projects.values().filter(|p|arr(p,"members").iter().any(|m|s(m,"bot_id")==bot_id)){let chat_id=s(project,"chat_id").to_string();body=body.child(Button::new(SharedString::from(format!("group-{}",s(project,"id")))).ghost().label(s(project,"name").to_string()).on_click(cx.listener(move|this,_,window,cx|this.select_chat(chat_id.clone(),window,cx))));}
                body=body.child(tr("context.routines"));for routine in self.routines.iter().filter(|r|s(r,"bot_id")==bot_id){body=body.child(format!("◷ {}",s(routine,"name")));}
                let screen_bot=bot_id.to_owned();body=body.child(Button::new("bot-screen").ghost().label(tr("action.screen")).on_click(cx.listener(move|this,_,_,cx|this.open_computer(screen_bot.clone(),cx))));
                body=body.child(Button::new("bot-settings").ghost().icon(IconName::Settings).label(tr("nav.settings")).on_click(cx.listener(|this,_,_,cx|this.navigate("bot_settings",cx))));
            }
        }
        div().size_full().flex().flex_col().bg(t.sidebar).child(header).child(body).into_any_element()
    }
    fn http_base(&self,cx:&App)->String {
        let endpoint=self.address.read(cx).value().to_string();let first=endpoint.split(',').next().unwrap_or("").trim();let url=if first.starts_with("wss://"){first.replacen("wss://","https://",1)}else if first.starts_with("ws://"){first.replacen("ws://","http://",1)}else if first.starts_with("http"){first.to_string()}else{format!("http://{first}")};url.trim_end_matches('/').trim_end_matches("/ws").to_string()
    }
    fn download_file(&mut self,file:Value,cx:&mut Context<Self>) {
        let base=self.http_base(cx);let password=self.password.read(cx).value().to_string();let path=s(&file,"path").to_owned();let name=std::path::Path::new(&path).file_name().and_then(|s|s.to_str()).unwrap_or("artifact").to_string();let root=s(&file,"root").to_string();let root_id=s(&file,"root_id").to_string();
        let task=self.runtime.spawn(async move{let response=reqwest::Client::new().get(format!("{base}/api/v1/files")).bearer_auth(password).query(&[("root",root),("root_id",root_id),("path",path)]).send().await?.error_for_status()?;let bytes=response.bytes().await?;let dir=std::env::temp_dir().join("MacBot").join(uuid::Uuid::new_v4().to_string());std::fs::create_dir_all(&dir)?;let target=dir.join(name);std::fs::write(&target,&bytes)?;Ok::<_,anyhow::Error>(target)});
        cx.spawn(async move |this,cx|{let result=task.await;let _=this.update(cx,|view,cx|match result{Ok(Ok(path))=>{let _=std::process::Command::new("open").arg(path).spawn();},Ok(Err(error))=>{view.notice=error.to_string();cx.notify();},Err(error)=>{view.notice=error.to_string();cx.notify();}});}).detach();
    }
    fn attach(&mut self,window:&mut Window,cx:&mut Context<Self>) {
        let paths=cx.prompt_for_paths(PathPromptOptions{files:true,directories:false,multiple:false,prompt:Some(tr("chat.attach"))});let base=self.http_base(cx);let password=self.password.read(cx).value().to_string();let runtime=self.runtime.handle().clone();
        cx.spawn_in(window,async move |this,cx|{if let Ok(Ok(Some(paths)))=paths.await{if let Some(path)=paths.first(){let path=path.clone();let task=runtime.spawn(async move{let name=path.file_name().and_then(|s|s.to_str()).unwrap_or("attachment").to_string();let bytes=std::fs::read(&path)?;if bytes.len()>100*1024*1024{anyhow::bail!("Attachment exceeds 100 MB");}let form=reqwest::multipart::Form::new().part("file",reqwest::multipart::Part::bytes(bytes).file_name(name));let result=reqwest::Client::new().post(format!("{base}/api/v1/uploads")).bearer_auth(password).multipart(form).send().await?.error_for_status()?.json::<Value>().await?;Ok::<_,anyhow::Error>(result)});let result=task.await;let _=this.update_in(cx,|view,_,cx|{match result{Ok(Ok(value))=>{view.attachments.push(s(&value,"upload_id").to_string());view.notice=tr("notice.attachment").to_string();},Ok(Err(error))=>view.notice=error.to_string(),Err(error)=>view.notice=error.to_string()}cx.notify();});}}}).detach();
    }
}
