use std::collections::BTreeMap;
use gpui_kit::*;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::component::{button::*, input::{Input,InputState,Textarea,TextareaState,InputEvent}, resizable::*, text::TextView, *};
use macbot_client_core::{AppState,Client,ClientConfig,ClientEvent,TraceTimeline};
use serde_json::{Value,json};
use crate::host_storage::HostStore;
use crate::computer::{Computer,ComputerAction};
use macbot_client_core::{ScreenClient,ScreenHandle,ScreenEvent};
use crate::{i18n::{tr,state},tokens::Tokens};

#[path = "chat.rs"] mod chat;

gpui_kit::actions!(macbot, [Quit,New,Search,Settings,MainBot,Workbench,Dashboard,Skills,ToggleSidebar,ToggleContext,Back]);

pub struct MacBot {
    computer: Entity<Computer>,
    screen_client: Option<ScreenClient>,
    screen_task: Option<Task<()>>,
    screen_bot: String,
    hosts: Option<HostStore>,
    active_host: Option<String>,
    runtime: tokio::runtime::Runtime,
    client: Option<Client>,
    event_task: Option<Task<()>>,
    state: AppState,
    address: Entity<InputState>,
    password: Entity<InputState>,
    host_name: Entity<InputState>,
    composer: Entity<TextareaState>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
    connected: bool,
    connecting: bool,
    fixture: bool,
    selected_chat: String,
    page: String,
    context: Vec<String>,
    sidebar_visible: bool,
    context_visible: bool,
    show_completed: bool,
    show_hidden: bool,
    notice: String,
    drafts: BTreeMap<String,String>,
    routines: Vec<Value>,
    templates: Vec<Value>,
    timeline: TraceTimeline,
    trace_target: Value,
    trace_stream: Option<String>,
    thread: Option<Value>,
    reply_to: Option<String>,
    changes_project: Option<String>,
    attachments: Vec<String>,
    message_scroll: ScrollHandle,
}
impl MacBot {
    pub fn new(window:&mut Window,cx:&mut Context<Self>)->Self {
        let address=cx.new(|cx|InputState::new(window,cx).placeholder("127.0.0.1:7788"));
        let password=cx.new(|cx|InputState::new(window,cx).masked(true));
        let host_name=cx.new(|cx|InputState::new(window,cx).placeholder("Mac mini"));
        let composer=cx.new(|cx|TextareaState::new(window,cx).placeholder(tr("chat.placeholder")).auto_grow(1,6).submit_on_enter(true));
        let computer=cx.new(|cx|Computer::new(window,cx));
        let mut subscriptions=vec![cx.subscribe_in(&composer,window,|this,_,event,window,cx| {
            if let InputEvent::PressEnter{shift:false,..}=event {this.send_message(window,cx);}
            if matches!(event,InputEvent::Change){this.drafts.insert(this.selected_chat.clone(),this.composer.read(cx).value().to_string());cx.notify();}
        })];
        subscriptions.push(cx.subscribe(&computer,|this,_,event:&ComputerAction,cx|this.computer_action(event,cx)));
        let mut view=Self { computer,screen_client:None,screen_task:None,screen_bot:String::new(),hosts:HostStore::load().ok(),active_host:None,runtime:tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("Tokio runtime"),
            client:None,event_task:None,state:AppState::default(),address,password,host_name,composer,focus:cx.focus_handle(),_subscriptions:subscriptions,
            connected:false,connecting:false,fixture:false,selected_chat:String::new(),page:"connect".into(),context:vec![],sidebar_visible:true,context_visible:true,
            show_completed:false,show_hidden:false,notice:String::new(),drafts:BTreeMap::new(),
            routines:vec![],templates:vec![],timeline:TraceTimeline::default(),trace_target:Value::Null,trace_stream:None,thread:None,reply_to:None,changes_project:None,attachments:vec![],message_scroll:ScrollHandle::new() };
        if std::env::var_os("MACBOT_HOST").is_none() {
            if let Some(host)=view.hosts.as_ref().and_then(|store|store.hosts().first().cloned()) {view.activate_host(&host.id,window,cx);}
        }
        if let Ok(endpoint)=std::env::var("MACBOT_HOST") {
            view.address.update(cx,|s,cx|s.set_value(endpoint,window,cx));
            let pw=std::env::var("MACBOT_PASSWORD").unwrap_or_default();
            view.password.update(cx,|s,cx|s.set_value(pw,window,cx));
            view.connect(cx);
        }
        if std::env::var_os("MACBOT_FIXTURES").is_some(){view.load_fixtures(cx);}
        view
    }
    fn connect(&mut self,cx:&mut Context<Self>) {
        let endpoint=self.address.read(cx).value().to_string();
        let password=self.password.read(cx).value().to_string();
        if endpoint.trim().is_empty(){self.notice=tr("error.empty").to_string();cx.notify();return;}
        if let Some(client)=self.client.take(){let _guard=self.runtime.enter();self.runtime.spawn(async move{client.close().await;});}
        self.connected=false;self.connecting=true;self.fixture=false;self.notice.clear();
        let addresses:Vec<String>=endpoint.split(',').map(|s|s.trim().to_string()).filter(|s|!s.is_empty()).collect();
        let mut config=ClientConfig::new(addresses.first().cloned().unwrap_or_default(),password);
        config.addresses=addresses;
        if let Some(store)=&self.hosts {
            config.device_id=store.device_id().into();
            if let Some(id)=&self.active_host {if let Some(host)=store.get(id){config.node_id=host.node_id.clone();}}
        }
        self.state.messages.clear();self.state.assignments.clear();self.state.announcements.clear();self.state.approvals.clear();self.state.questions.clear();self.state=AppState::default();
        self.selected_chat.clear();self.context.clear();
        let _guard=self.runtime.enter();
        let handle=Client::spawn(config);
        self.client=Some(handle.client);
        let mut events=handle.events;
        self.event_task=Some(cx.spawn(async move |this,cx| {
            while let Some(event)=events.recv().await {
                if this.update(cx,|view,cx|view.on_event(event,cx)).is_err(){break;}
            }
        }));
        cx.notify();
    }
    fn rpc(&mut self,method:&str,params:Value,cx:&mut Context<Self>) {
        let Some(client)=self.client.as_ref() else{self.notice=tr("error.offline").to_string();cx.notify();return;};
        let method=method.to_owned();
        let params_copy=params.clone();
        let receiver={let _guard=self.runtime.enter();client.try_request(method.clone(),params)};
        cx.spawn(async move |this,cx| {
            let result=receiver.await;
            let _=this.update(cx,|view,cx|{
                match result {Ok(Ok(value))=>view.rpc_result(&method,&params_copy,value,cx),Ok(Err(error))=>{view.notice=error.to_string();},Err(error)=>{view.notice=error.to_string();}}
                cx.notify();
            });
        }).detach();
    }
    fn on_event(&mut self,event:ClientEvent,cx:&mut Context<Self>) {
        match event {
            ClientEvent::Connected{hello,..}=>{self.state.hello=Some(hello);self.connected=true;self.connecting=false;self.page="chat".into();self.notice.clear();self.remember_host(cx);self.rpc("bot.templates",json!({}),cx);self.refresh(cx);},
            ClientEvent::Bootstrap(value)=>{self.state.apply_bootstrap(value);if self.selected_chat.is_empty(){self.select_main(cx);}},
            ClientEvent::Protocol(event)=>{
                let d=&event.data;
                match event.event.as_str(){
                    "trace.item"=>{if self.trace_stream.as_deref()==Some(s(d,"stream")){self.timeline.apply_item(d["item"].clone());}},
                    "trace.delta"=>{if self.trace_stream.as_deref()==Some(s(d,"stream")){self.timeline.apply_delta(s(d,"request_id"),s(d,"channel"),s(d,"text"));}},
                    _=>{}
                }
                self.state.apply_event(event);
            },
            ClientEvent::Disconnected{error}=>{self.connected=false;self.connecting=false;self.trace_stream=None;if let Some(e)=error{self.notice=e;}},
            ClientEvent::TransportError(error)=>{self.notice=error;self.connecting=false;}
        }
        cx.notify();
    }
    fn rpc_result(&mut self,method:&str,params:&Value,value:Value,cx:&mut Context<Self>) {
        if let Some(message)=value.get("message"){insert(&mut self.state.messages,message,"id");}
        if let Some(chat)=value.get("chat").or_else(||value.get("dm_chat")){insert(&mut self.state.chats,chat,"id");}
        if let Some(bot)=value.get("bot"){insert(&mut self.state.bots,bot,"id");}
        if let Some(project)=value.get("project"){insert(&mut self.state.projects,project,"id");}
        match method {
            "chat.history"=>{for v in arr(&value,"messages"){insert(&mut self.state.messages,v,"id");}},
            "assignment.list"=>{for v in arr(&value,"items"){insert(&mut self.state.assignments,v,"id");}},
            "project.get"=>{if let Some(v)=value.get("announcement"){insert(&mut self.state.announcements,v,"project_id");}},
            "approval.list"=>{for v in arr(&value,"approvals"){insert(&mut self.state.approvals,v,"id");}},
            "routine.list"=>{self.routines=arr(&value,"routines").to_vec();},
            "bot.templates"=>self.templates=arr(&value,"templates").to_vec(),
            "chat.thread"=>{self.thread=Some(value);self.context.push("thread".into());self.context_visible=true;},
            "trace.history"=>{
                self.timeline.apply_history(&value);
                if self.timeline.live&&self.trace_stream.is_none(){let mut target=self.trace_target.clone();target["since_aseq"]=json!(self.timeline.last_aseq.unwrap_or(0));self.rpc("trace.subscribe",target,cx);}
            },
            "trace.subscribe"=>{self.trace_stream=Some(s(&value,"stream").into());for v in arr(&value,"in_flight"){self.timeline.apply_delta(s(v,"request_id"),"text",s(v,"text"));self.timeline.apply_delta(s(v,"request_id"),"thinking",s(v,"thinking"));}},
            "chat.send"=>{self.notice=tr("notice.sent").to_string();},
            "project.create"=>{if let Some(chat)=value.get("chat"){self.selected_chat=s(chat,"id").into();self.page="chat".into();}},
            _=>{let _=params;}
        }
    }
    fn refresh(&mut self,cx:&mut Context<Self>){self.rpc("assignment.list",json!({}),cx);self.rpc("approval.list",json!({}),cx);self.rpc("routine.list",json!({}),cx);}
    fn load_fixtures(&mut self,cx:&mut Context<Self>) {
        let root=std::env::var("MACBOT_FIXTURES").unwrap_or_else(|_|{
            let bundled=std::env::current_exe().ok().and_then(|p|p.parent().and_then(|p|p.parent()).map(|p|p.join("Resources/fixtures")));
            bundled.filter(|p|p.is_dir()).map(|p|p.display().to_string()).unwrap_or_else(||format!("{}/../../../../protocol/fixtures",env!("CARGO_MANIFEST_DIR")))
        });
        let read=|name:&str|std::fs::read_to_string(std::path::Path::new(&root).join(name)).ok().and_then(|text|serde_json::from_str::<Value>(&text).ok());
        let mut value=read("bootstrap.json").or_else(||read("objects/bootstrap.json"));
        if value.is_none(){
            if let Some(hello)=read("objects/hello.json"){
                let mut bootstrap=json!({"seq":0,"hello":hello,"settings":read("objects/settings.json"),"pending":{}});
                for (kind,key) in [("bot","bots"),("chat","chats"),("project","projects"),("message","messages"),("assignment","assignments"),("announcement","announcements"),("approval","approvals"),("question","questions"),("routine","routines"),("skill","skills"),("provider","providers"),("model","models")]{bootstrap[key]=json!(read(&format!("objects/{kind}.json")).into_iter().collect::<Vec<_>>());}
                value=Some(bootstrap);
            }
        }
        if let Some(value)=value{
            self.state.apply_bootstrap(value);self.fixture=true;self.connected=false;self.page="chat".into();self.select_main(cx);self.notice=tr("status.fixture").to_string();self.routines=self.state.routines.values().cloned().collect();cx.notify();
        }else{self.notice=tr("connect.fixture_missing").to_string();cx.notify();}
    }
    fn remember_host(&mut self,cx:&mut Context<Self>){
        let Some(hello)=self.state.hello.as_ref() else{return;};
        let name=self.host_name.read(cx).value().to_string();
        let name=if name.trim().is_empty(){s(hello,"host_name").to_owned()}else{name};
        let addresses=self.address.read(cx).value().split(',').map(|s|s.trim().to_string()).collect();
        let password=self.password.read(cx).value().to_string();
        if let Some(store)=self.hosts.as_mut(){match store.remember(name,addresses,&password,Some(s(hello,"node_id").into()),self.state.last_seq){Ok(host)=>self.active_host=Some(host.id),Err(error)=>self.notice=error.to_string()}}
    }
    fn activate_host(&mut self,id:&str,window:&mut Window,cx:&mut Context<Self>){
        if let Some(store)=&self.hosts {if let Some(record)=store.get(id).cloned(){match store.password(&record){Ok(password)=>{
            self.active_host=Some(record.id);self.address.update(cx,|s,cx|s.set_value(record.addresses.join(", "),window,cx));self.host_name.update(cx,|s,cx|s.set_value(record.name,window,cx));self.password.update(cx,|s,cx|s.set_value(password,window,cx));self.connect(cx);
        },Err(error)=>{self.notice=error.to_string();cx.notify();}}}}
    }
    fn select_main(&mut self,cx:&mut Context<Self>){if let Some(chat)=self.state.chats.values().find(|v|s(v,"kind")=="main"){let id=s(chat,"id").to_owned();self.selected_chat=id.clone();self.page="chat".into();if self.connected{self.rpc("chat.history",json!({"chat_id":id}),cx);}}}
    fn select_chat(&mut self,id:String,window:&mut Window,cx:&mut Context<Self>){
        self.close_trace(cx);self.selected_chat=id.clone();self.page="chat".into();self.context.clear();self.reply_to=None;self.thread=None;self.attachments.clear();
        let draft=self.drafts.get(&id).cloned().unwrap_or_default();self.composer.update(cx,|input,cx|input.set_value(draft,window,cx));
        self.rpc("chat.history",json!({"chat_id":id}),cx);
        if let Some(chat)=self.state.chats.get(&self.selected_chat){let seq=chat["last_seq"].clone();let project=chat["project_id"].clone();self.rpc("chat.mark_read",json!({"chat_id":self.selected_chat,"seq":seq}),cx);if !project.is_null(){self.rpc("project.get",json!({"project_id":project}),cx);}}
        cx.notify();
    }
    fn open_computer(&mut self,bot:String,cx:&mut Context<Self>){
        self.close_screen();self.screen_bot=bot.clone();self.page="computer".into();
        let addresses=self.address.read(cx).value().to_string();let endpoint=addresses.split(',').next().unwrap_or("");
        let config=ClientConfig::new(endpoint,self.password.read(cx).value().to_string());
        let quality=self.computer.read(cx).quality().to_string();
        let _guard=self.runtime.enter();let handle=ScreenHandle::spawn(config,bot,quality,None);self.screen_client=Some(handle.client);let mut events=handle.events;
        self.screen_task=Some(cx.spawn(async move |this,cx| {while let Some(event)=events.recv().await{if this.update(cx,|view,cx|{match event{
            ScreenEvent::State(state)=>view.computer.update(cx,|screen,cx|screen.set_state_in(state,cx)),
            ScreenEvent::Frame(frame)=>view.computer.update(cx,|screen,cx|screen.set_frame(frame.header.seq,frame.header.w,frame.header.h,frame.jpeg,cx)),
            ScreenEvent::Error(error)=>view.notice=error,
            ScreenEvent::Closed=>view.notice=tr("status.disconnected").to_string(),
        }cx.notify();}).is_err(){break;}}}));cx.notify();
    }
    fn close_screen(&mut self){self.screen_task=None;if let Some(screen)=self.screen_client.take(){self.runtime.spawn(async move{let _=screen.close().await;});}}
    fn computer_action(&mut self,event:&ComputerAction,cx:&mut Context<Self>){
        match event {
            ComputerAction::Takeover=>self.rpc("takeover.start",json!({"bot_id":self.screen_bot}),cx),
            ComputerAction::Release=>self.rpc("takeover.release",json!({"bot_id":self.screen_bot}),cx),
            ComputerAction::Quality(_)=>{let bot=self.screen_bot.clone();self.open_computer(bot,cx);},
            _=>{if let Some(screen)=self.screen_client.as_ref().cloned(){let event=event.clone();self.runtime.spawn(async move{match event{
                ComputerAction::Rendered(seq)=>{let _=screen.ack(seq).await;},
                ComputerAction::SwitchTab(tab)=>{let _=screen.switch_tab(tab).await;},
                ComputerAction::Input(value)=>{let _=screen.input(value.get("event").cloned().unwrap_or(value)).await;},
                _=>{}
            }});}}
        }
    }
    fn close_trace(&mut self,cx:&mut Context<Self>){if let Some(stream)=self.trace_stream.take(){self.rpc("trace.unsubscribe",json!({"stream":stream}),cx);}}
    fn open_trace(&mut self,id:Option<String>,cx:&mut Context<Self>){self.close_trace(cx);self.timeline=TraceTimeline::default();self.trace_target=if let Some(id)=id{json!({"assignment_id":id})}else{json!({"chat_id":self.selected_chat})};let mut params=self.trace_target.clone();params["tail"]=json!(true);self.rpc("trace.history",params,cx);self.context.push("trace".into());self.context_visible=true;cx.notify();}
    fn navigate(&mut self,page:&str,cx:&mut Context<Self>){self.close_trace(cx);self.close_screen();self.page=page.into();cx.notify();}
    fn back(&mut self,window:&mut Window,cx:&mut Context<Self>){if !self.context.is_empty(){self.context.pop();self.close_trace(cx);}else{self.page=if self.client.is_some()||self.fixture{"chat"}else{"connect"}.into();}self.focus.focus(window,cx);cx.notify();}
    fn sidebar(&self,cx:&mut Context<Self>)->AnyElement {
        let t=Tokens::get(cx);
        let mut list=div().id("chat-list").flex_1().min_h_0().overflow_y_scroll().px_3().py_2().flex().flex_col().gap_1();
        for chat in self.state.chats.values().filter(|v|s(v,"kind")=="main") {list=list.child(self.chat_row(chat,cx));}
        list=list.child(div().mt_4().mb_1().px_2().text_xs().text_color(t.secondary).child(tr("nav.groups")));
        let mut groups:Vec<_>=self.state.chats.values().filter(|v|s(v,"kind")=="project").collect();groups.sort_by_key(|v|(!v["pinned"].as_bool().unwrap_or(false),s(v,"updated_at").to_string()));
        let mut done=0;
        for chat in groups {let status=self.state.projects.get(s(chat,"project_id")).map(|v|s(v,"status")).unwrap_or("active");if status=="archived"{continue;}if status=="done"{done+=1;if !self.show_completed{continue;}}list=list.child(self.chat_row(chat,cx));}
        if !self.state.chats.values().any(|v|s(v,"kind")=="project"){list=list.child(div().px_2().py_2().text_xs().text_color(t.secondary).child(tr("nav.empty_group")));}
        if done>0{list=list.child(Button::new("completed").ghost().small().label(format!("{} {done}",tr("nav.done"))).on_click(cx.listener(|this,_,_,cx|{this.show_completed=!this.show_completed;cx.notify();})));}
        list=list.child(div().mt_4().mb_1().px_2().text_xs().text_color(t.secondary).child(tr("nav.bots")));
        for bot in self.state.bots.values().filter(|v|v["is_main"]!=true){if bot["hidden"]==true&&!self.show_hidden{continue;}let id=s(bot,"dm_chat_id");if let Some(chat)=self.state.chats.get(id){list=list.child(self.chat_row(chat,cx));}}
        let hidden=self.state.bots.values().filter(|b|b["hidden"]==true).count();
        if hidden>0 {list=list.child(Button::new("hidden").ghost().small().label(format!("{} {hidden}",tr("nav.hidden"))).on_click(cx.listener(|this,_,_,cx|{this.show_hidden=!this.show_hidden;cx.notify();})));}
        let host=self.state.hello.as_ref().map(|v|s(v,"host_name")).filter(|s|!s.is_empty()).unwrap_or("Mac Bot").to_string();
        let mut footer=div().flex().flex_col().gap_1().p_3().border_t_1().border_color(t.border);
        for (key,icon) in [("workbench",IconName::LayoutDashboard),("dashboard",IconName::LayoutDashboard),("skills",IconName::Star),("settings",IconName::Settings)]{
            let page=key.to_string();footer=footer.child(Button::new(SharedString::from(format!("nav-{key}"))).ghost().icon(icon).label(tr(&format!("nav.{key}"))).on_click(cx.listener(move|this,_,_,cx|this.navigate(&page,cx))));
        }
        div().flex().flex_col().size_full().bg(t.sidebar)
            .child(div().p_3().flex().items_center().gap_2().child(div().size(px(7.)).rounded_full().bg(if self.connected{t.success}else{t.attention})).child(Button::new("host-switch").ghost().label(host).on_click(cx.listener(|this,_,_,cx|this.navigate("connect",cx)))).child(Button::new("new").ghost().icon(IconName::Plus).tooltip(tr("nav.new")).on_click(cx.listener(|this,_,_,cx|this.navigate("new",cx)))))
            .child(Button::new("search").ghost().icon(IconName::Search).label(tr("nav.search")).on_click(cx.listener(|this,_,_,cx|this.navigate("search",cx))))
            .child(list).child(footer).into_any_element()
    }
    fn chat_row(&self,chat:&Value,cx:&mut Context<Self>)->AnyElement {
        let t=Tokens::get(cx);let id=s(chat,"id").to_owned();let selected=id==self.selected_chat&&self.page=="chat";
        let bot=self.state.bots.get(s(chat,"bot_id"));let title=s(chat,"title").to_owned();let preview=s(&chat["last_message"],"text").to_owned();
        let badge=if chat["unread"].as_u64().unwrap_or(0)>0{format!(" · {}",chat["unread"])}else{String::new()};
        div().rounded(px(10.)).bg(if selected{t.bot}else{t.sidebar}).p_1().child(
            Button::new(SharedString::from(id.clone())).ghost().w_full().child(div().flex().items_center().gap_3().w_full().child(bean(bot.unwrap_or(&Value::Null),32.,cx)).child(div().flex().flex_col().items_start().gap_1().flex_1().min_w_0().child(div().text_sm().child(title+&badge)).child(div().text_xs().text_color(t.secondary).truncate().child(preview)))).on_click(cx.listener(move|this,_,window,cx|this.select_chat(id.clone(),window,cx)))
        ).into_any_element()
    }
    fn connection_page(&self,cx:&mut Context<Self>)->AnyElement {
        let t=Tokens::get(cx);
        let field=|key:&str,input:&Entity<InputState>|div().flex().flex_col().gap_2().child(div().text_sm().child(tr(key))).child(Input::new(input));
        div().size_full().flex().items_center().justify_center().bg(t.window).child(div().flex().gap_8().max_w(px(840.)).p_8()
            .child(div().w(px(380.)).flex().flex_col().gap_5().child(div().text_2xl().font_weight(FontWeight::SEMIBOLD).child(tr("connect.welcome")))
            .child(div().text_color(t.secondary).child(tr("connect.title"))).child(field("connect.address",&self.address)).child(div().text_xs().text_color(t.secondary).child(tr("connect.hint")))
            .when(self.hosts.as_ref().is_some_and(|store|!store.hosts().is_empty()),|el|el.child(div().flex().flex_col().gap_1().child(tr("connect.saved")).children(self.hosts.as_ref().unwrap().hosts().iter().map(|host|{let id=host.id.clone();Button::new(SharedString::from(format!("saved-{id}"))).ghost().small().label(host.name.clone()).on_click(cx.listener(move|this,_,window,cx|this.activate_host(&id,window,cx)))}))))
            .child(field("connect.password",&self.password)).child(field("connect.name",&self.host_name))
            .child(Button::new("connect").primary().label(tr(if self.connecting{"connect.connecting"}else{"connect.action"})).loading(self.connecting).on_click(cx.listener(|this,_,_,cx|this.connect(cx))))
            .child(Button::new("fixtures").ghost().label(tr("connect.fixture")).on_click(cx.listener(|this,_,_,cx|this.load_fixtures(cx)))))
            .child(div().w(px(300.)).flex().flex_col().gap_5().pt_8().child(bean(&json!({"avatar":{"color":0},"is_main":true}),64.,cx)).child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(tr("connect.main"))).child(tr("connect.description")).child(div().mt_6().text_color(t.secondary).child(tr("connect.team"))).children(self.templates.iter().map(|template|{let id=s(template,"id").to_string();Button::new(SharedString::from(id.clone())).outline().label(s(template,"name").to_string()).on_click(cx.listener(move|this,_,_,cx|this.rpc("bot.create_from_template",json!({"template_id":id}),cx)))}))))
            .into_any_element()
    }
}
impl Render for MacBot {
    fn render(&mut self,window:&mut Window,cx:&mut Context<Self>)->impl IntoElement {
        let t=Tokens::get(cx);
        let center=if self.page=="computer"{self.computer.clone().into_any_element()}else if self.page=="connect"{self.connection_page(cx)}else if self.page=="chat"{self.chat_page(window,cx)}else{div().flex().flex_col().p_6().gap_4().child(tr(&format!("nav.{}",self.page))).child(Button::new("back-page").ghost().label(tr("action.back")).on_click(cx.listener(|this,_,window,cx|this.back(window,cx)))).into_any_element()};
        div().size_full().flex().flex_col().bg(t.window).text_color(t.primary).text_size(px(14.)).track_focus(&self.focus)
            .on_action(cx.listener(|this,_:&New,_,cx|this.navigate("new",cx)))
            .on_action(cx.listener(|this,_:&Search,_,cx|this.navigate("search",cx)))
            .on_action(cx.listener(|this,_:&Settings,_,cx|this.navigate("settings",cx)))
            .on_action(cx.listener(|this,_:&MainBot,_,cx|this.select_main(cx)))
            .on_action(cx.listener(|this,_:&Workbench,_,cx|this.navigate("workbench",cx)))
            .on_action(cx.listener(|this,_:&Dashboard,_,cx|this.navigate("dashboard",cx)))
            .on_action(cx.listener(|this,_:&Skills,_,cx|this.navigate("skills",cx)))
            .on_action(cx.listener(|this,_:&ToggleSidebar,_,cx|{this.sidebar_visible=!this.sidebar_visible;cx.notify();}))
            .on_action(cx.listener(|this,_:&ToggleContext,_,cx|{this.context_visible=!this.context_visible;cx.notify();}))
            .on_action(cx.listener(|this,_:&Back,window,cx|this.back(window,cx)))
            .when(!self.notice.is_empty(),|el|el.child(div().px_4().py_2().bg(t.sidebar).text_xs().text_color(t.secondary).child(self.notice.clone())))
            .child(h_resizable("main-panes")
                .when(self.sidebar_visible,|el|el.child(resizable_panel().size(px(280.)).size_range(px(200.)..px(400.)).child(self.sidebar(cx))))
                .child(resizable_panel().size(px(660.)).size_range(px(480.)..px(2000.)).child(center))
                .when(self.context_visible&&self.page=="chat",|el|el.child(resizable_panel().size(px(340.)).size_range(px(260.)..px(600.)).child(self.context_panel(window,cx)))))
    }
}
fn s<'a>(v:&'a Value,key:&str)->&'a str{v.get(key).and_then(Value::as_str).unwrap_or("")}
fn arr<'a>(v:&'a Value,key:&str)->&'a [Value]{v.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])}
fn insert(map:&mut BTreeMap<String,Value>,value:&Value,key:&str){let id=s(value,key);if !id.is_empty(){map.insert(id.to_string(),value.clone());}}
fn bean(bot:&Value,size:f32,cx:&App)->AnyElement {
    let t=Tokens::get(cx);let avatar=&bot["avatar"];let color=avatar["color"].as_u64().unwrap_or(0);
    let body=div().size(px(size)).rounded_full().flex().items_center().justify_center().bg(Tokens::bean(color)).text_color(t.user_text)
        .child(if s(avatar,"kind")=="emoji"{s(avatar,"emoji").to_string()}else{"● ●".into()});
    div().relative().flex_none().child(body).when(bot["is_main"]==true,|el|el.child(div().absolute().top(px(-8.)).right(px(0.)).text_size(px(14.)).child("♛"))).into_any_element()
}
