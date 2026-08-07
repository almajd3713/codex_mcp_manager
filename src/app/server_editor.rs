use super::*;

impl<'a> App<'a> {
    pub(super) fn handle_edit_confirmation(
        &mut self,
        key: KeyEvent,
        server_name: &str,
        selected: usize,
    ) -> Result<Option<Outcome>, StoreError> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let server = self.store.load_server_definition(server_name)?;
                self.screen = Screen::ServerForm(ServerForm::for_edit(
                    server,
                    self.catalog.base_fingerprint,
                    selected,
                ));
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.screen = Screen::Inventory { selected };
            }
            _ => {
                self.screen = Screen::ConfirmEdit {
                    server_name: server_name.to_owned(),
                    selected,
                };
            }
        }
        Ok(None)
    }
}
