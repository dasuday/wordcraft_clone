//! Hand-written synthetic Greek test data. No words, rules or suggestion tables were copied
//! from a system dictionary. This is a small fixture, not a redistributable Greek dictionary.

pub const AFF: &str = "SET UTF-8
FLAG UTF-8
FORBIDDENWORD F
TRY αεοιηυωνστκπρλμδγχθφβζξψάέόίήύώς
KEY ερτυθιοπ|ασδφγηξκλ|ζχψωβνμ
SFX A Y 1 # synthetic plural -ια
SFX A 0 α ι
SFX B Y 1
SFX B ος οι ος
SFX C Y 3
SFX C ός ά ός
SFX C ός ό ός
SFX C ός ή ός
";

pub const DIC: &str = "31
σπίτι/A
λόγος/B
καλημέρα
Αθήνα
Ελλάδα
είναι
και
ελληνικός/C
αγγλικός/C
από
για
όλα
όλη
αυτό
το
τη
την
στη
στην
μου
ήρθε
πόλη
πολύ
όμορφο
μητέρα
ώρα
προϊόν
δεν
σε
βλέπω
ζξψαπαγορευμένο/F
";

pub fn install() {
    if let Err(e) = super::install_dictionary(AFF, DIC) {
        panic!("test dictionary: {e}");
    }
}
