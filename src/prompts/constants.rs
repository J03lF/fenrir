pub(super) const CLEAR_SCREEN: &str = "\x1B[2J\x1B[H";
pub(super) const COLOR_RESET: &str = "\x1b[0m";
pub(super) const COLOR_PRIMARY: &str = "\x1b[38;5;39m";
pub(super) const COLOR_ACCENT: &str = "\x1b[38;5;214m";
pub(super) const COLOR_DIM: &str = "\x1b[38;5;244m";
pub(super) const COLOR_PROMPT: &str = "\x1b[38;5;47m";

pub(super) const RAW_BANNER: &str = r#"
                  =
                ++=-**+
               *+=#=***
             *#*+=%*+=---
            ***#++===+++=--
           *#**%%*+++++*#*==
           #************++++==*
          *#**%#***##%%%##****%
          *%##%*%%%%@    %%%%
          +#%#%%%%%
           ##*%%%%###%%%
            **%%%%%%%%%
              %%%%%
               %%%%
               %%%
               %%
"#;
